//! Recurrence resolution helpers shared by the events subsystem.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc, Weekday};

use crate::models::enums::RecurrenceType;

/// The recurrence cadence of a fact: the kind, the raw `RRULE` (when the
/// producer supplied one — interval, day/month constraints, and
/// `COUNT`/`UNTIL` verbatim), the interval (every N periods), and the
/// effective series end.
#[derive(Debug, Clone, PartialEq)]
pub struct RecurrenceSpec {
    pub kind: RecurrenceType,
    pub rule: Option<String>,
    pub interval: i32,
    pub until: Option<DateTime<Utc>>,
}

/// Compute the next occurrence of a recurring date on or after `from`.
///
/// - `None` → returns the original `date_value` parsed as a one-time date.
/// - `Yearly` → next anniversary; Feb 29 falls back to Mar 1 in non-leap years.
/// - `Monthly` → same day each month; if day exceeds month length, falls back to last day.
/// - `Weekly` → same weekday each week.
/// - `Daily` → every day (returns `from` truncated to midnight if `date_value` is date-only,
///   or `from` if datetime).
///
/// `interval` repeats the period every N steps (2 = fortnightly for `Weekly`,
/// every other month for `Monthly`, ...); `until` bounds the series — once the
/// next occurrence would fall after it, `None` is returned so the scan stops
/// advancing the overlay (a bounded series no longer stays active
/// indefinitely). `rule` carries the raw `RRULE` so the stored day/month
/// constraints are evaluated: `BYDAY` selects the weekdays of a `Weekly`
/// series, `BYMONTHDAY` the day of an absolute monthly/yearly series,
/// `BYMONTH` the month of a yearly series, and `BYDAY` + `BYSETPOS` the Nth
/// weekday of a relative monthly/yearly series.
pub fn next_occurrence(
    date_value: &str,
    recurrence: RecurrenceType,
    interval: i32,
    until: Option<DateTime<Utc>>,
    from: DateTime<Utc>,
    rule: Option<&str>,
) -> Option<DateTime<Utc>> {
    let base = parse_base_datetime(date_value)?;
    let interval = interval.max(1) as i64;
    let constraints = parse_rrule_constraints(rule);

    let next = match recurrence {
        RecurrenceType::None => base,
        RecurrenceType::Daily => {
            let base_date = base.date_naive();
            let from_date = from.date_naive();
            if base_date > from_date {
                base
            } else {
                let days_ahead = (from_date - base_date).num_days();
                let mut candidate = base + Duration::days((days_ahead / interval) * interval);
                if candidate < from {
                    candidate += Duration::days(interval);
                }
                candidate
            }
        }
        RecurrenceType::Weekly => {
            if !constraints.by_day.is_empty() {
                next_weekly_byday(base, interval, from, &constraints.by_day)?
            } else if from.date_naive() < base.date_naive() {
                base
            } else {
                let from_weekday = from.date_naive().weekday().num_days_from_monday() as i64;
                let base_weekday = base.date_naive().weekday().num_days_from_monday() as i64;
                let mut days_ahead = (base_weekday - from_weekday + 7) % 7;
                if days_ahead == 0 && base.time() < from.time() {
                    days_ahead = 7;
                }
                let mut candidate = from.date_naive() + Duration::days(days_ahead);
                // Snap to the next valid week (every `interval` weeks from the
                // base week) so a fortnightly event does not advance weekly.
                let weeks = (candidate - base.date_naive()).num_days() / 7;
                if weeks % interval != 0 {
                    candidate += Duration::days((interval - weeks % interval) * 7);
                }
                Utc.from_local_datetime(&candidate.and_time(base.time()))
                    .single()?
            }
        }
        RecurrenceType::Monthly => {
            let base_date = base.date_naive();
            let months = (from.year() as i64 - base_date.year() as i64) * 12
                + (from.month() as i64 - base_date.month() as i64);
            if months < 0 {
                base
            } else {
                let k = months / interval;
                let mut candidate = month_candidate(base, k * interval, &constraints)?;
                if candidate < from {
                    candidate = month_candidate(base, (k + 1) * interval, &constraints)?;
                }
                candidate
            }
        }
        RecurrenceType::Yearly => {
            let base_date = base.date_naive();
            let years = from.year() as i64 - base_date.year() as i64;
            if years < 0 {
                base
            } else {
                let k = years / interval;
                let mut candidate = year_candidate(base, k * interval, &constraints)?;
                if candidate < from {
                    candidate = year_candidate(base, (k + 1) * interval, &constraints)?;
                }
                candidate
            }
        }
    };
    if let Some(until) = until {
        if next > until {
            return None;
        }
    }
    Some(next)
}

/// The candidate occurrence in the month `months_after` months after the base
/// month, honouring the rule constraints: a relative pattern (`BYDAY` +
/// `BYSETPOS`) resolves to the Nth weekday of the month, an absolute pattern
/// (`BYMONTHDAY`) to the day of the month, and a plain pattern to the base
/// day clamped to the month length.
fn month_candidate(
    base: DateTime<Utc>,
    months_after: i64,
    constraints: &RruleConstraints,
) -> Option<DateTime<Utc>> {
    let base_date = base.date_naive();
    let total = base_date.year() as i64 * 12 + (base_date.month() as i64 - 1) + months_after;
    let year = (total / 12) as i32;
    let month = (total % 12) as u32 + 1;
    let day =
        if let (Some(pos), Some(weekday)) = (constraints.by_set_pos, constraints.by_day.first()) {
            nth_weekday_of_month(year, month, *weekday, pos)?
        } else if let Some(day) = constraints.by_month_day.first() {
            month_day_candidate(year, month, *day)?
        } else {
            std::cmp::min(base_date.day(), days_in_month(year, month))
        };
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| Utc.from_local_datetime(&d.and_time(base.time())).single())
}

/// The candidate occurrence in the year `years_after` years after the base
/// year, honouring the rule constraints: `BYMONTH` selects the month (the
/// base month when absent), a relative pattern (`BYDAY` + `BYSETPOS`)
/// resolves to the Nth weekday of that month, `BYMONTHDAY` to the day of the
/// month, and a plain pattern to the base month/day (Feb 29 falls back to
/// Mar 1 in non-leap years).
fn year_candidate(
    base: DateTime<Utc>,
    years_after: i64,
    constraints: &RruleConstraints,
) -> Option<DateTime<Utc>> {
    let base_date = base.date_naive();
    let year = base_date.year() + years_after as i32;
    let month = constraints
        .by_month
        .first()
        .copied()
        .unwrap_or(base_date.month());
    let day =
        if let (Some(pos), Some(weekday)) = (constraints.by_set_pos, constraints.by_day.first()) {
            nth_weekday_of_month(year, month, *weekday, pos)?
        } else if let Some(day) = constraints.by_month_day.first() {
            month_day_candidate(year, month, *day)?
        } else {
            base_date.day()
        };
    let (month, day) = if month == 2 && day == 29 && !is_leap_year(year) {
        (3, 1)
    } else {
        (month, day)
    };
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| Utc.from_local_datetime(&d.and_time(base.time())).single())
}

/// The next occurrence of a multi-day weekly series: the next date on or
/// after `from` whose weekday is in the rule's `BYDAY` set and whose week is
/// a valid `INTERVAL` week (counted from the calendar week containing the
/// base date, per RFC 5545).
fn next_weekly_byday(
    base: DateTime<Utc>,
    interval: i64,
    from: DateTime<Utc>,
    by_day: &[Weekday],
) -> Option<DateTime<Utc>> {
    let base_date = base.date_naive();
    let from_date = from.date_naive();
    if from_date < base_date {
        return Some(base);
    }
    let base_week_start =
        base_date - Duration::days(base_date.weekday().num_days_from_monday() as i64);
    let from_week_start =
        from_date - Duration::days(from_date.weekday().num_days_from_monday() as i64);
    let from_week = (from_week_start - base_week_start).num_days() / 7;
    // The next valid `INTERVAL` week on or after `from`'s week.
    let mut week = from_week + ((interval - from_week % interval) % interval);
    loop {
        let week_start = base_week_start + Duration::days(week * 7);
        // The first BYDAY weekday of this week on or after `from`; only the
        // `from` day itself can fail the time check, so the loop runs at
        // most twice (this valid week, then the next one).
        for offset in 0..7 {
            let candidate_date = week_start + Duration::days(offset);
            if candidate_date < from_date {
                continue;
            }
            if !by_day.contains(&candidate_date.weekday()) {
                continue;
            }
            let candidate = Utc
                .from_local_datetime(&candidate_date.and_time(base.time()))
                .single()?;
            if candidate >= from {
                return Some(candidate);
            }
        }
        week += interval;
    }
}

/// The day of the Nth weekday of a month: `pos` 1-4 selects the 1st-4th
/// occurrence, negative values count from the end (`-1` = the last one).
/// Returns `None` when the month has no such occurrence (e.g. a 5th weekday
/// that does not exist).
fn nth_weekday_of_month(year: i32, month: u32, weekday: Weekday, pos: i32) -> Option<u32> {
    let dim = days_in_month(year, month);
    if dim == 0 {
        return None;
    }
    if pos > 0 {
        let first_weekday = NaiveDate::from_ymd_opt(year, month, 1)?.weekday();
        let offset = (weekday.num_days_from_monday() as i64
            - first_weekday.num_days_from_monday() as i64
            + 7)
            % 7;
        let day = 1 + offset as u32 + (pos as u32 - 1) * 7;
        if day > dim {
            return None;
        }
        Some(day)
    } else {
        let last_weekday = NaiveDate::from_ymd_opt(year, month, dim)?.weekday();
        let back = (last_weekday.num_days_from_monday() as i64
            - weekday.num_days_from_monday() as i64
            + 7)
            % 7
            + (pos.unsigned_abs() - 1) as i64 * 7;
        if back >= dim as i64 {
            return None;
        }
        Some(dim - back as u32)
    }
}

/// The day of the month for a `BYMONTHDAY` value: positive values count from
/// the first, negative values from the end (`-1` = last day). Values outside
/// the month length are clamped to the month's last day.
fn month_day_candidate(year: i32, month: u32, day: i32) -> Option<u32> {
    let dim = days_in_month(year, month);
    if dim == 0 {
        return None;
    }
    let resolved = if day > 0 {
        day as u32
    } else {
        (dim as i32 + day + 1) as u32
    };
    Some(resolved.clamp(1, dim))
}

/// The day/month constraints parsed from a raw `RRULE` (`BYDAY`,
/// `BYMONTHDAY`, `BYMONTH`, `BYSETPOS`). Unknown or unparseable values are
/// dropped; an empty set means the constraint is absent and the plain
/// kind/interval cadence applies.
#[derive(Debug, Clone, Default, PartialEq)]
struct RruleConstraints {
    by_day: Vec<Weekday>,
    by_month_day: Vec<i32>,
    by_month: Vec<u32>,
    by_set_pos: Option<i32>,
}

fn parse_rrule_constraints(rule: Option<&str>) -> RruleConstraints {
    let mut constraints = RruleConstraints::default();
    let Some(rule) = rule else {
        return constraints;
    };
    for part in rule.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_uppercase().as_str() {
            "BYDAY" => {
                constraints.by_day = value
                    .split(',')
                    .filter_map(|d| weekday_from_rrule(d.trim()))
                    .collect();
            }
            "BYMONTHDAY" => {
                constraints.by_month_day = value
                    .split(',')
                    .filter_map(|d| d.trim().parse::<i32>().ok())
                    .collect();
            }
            "BYMONTH" => {
                constraints.by_month = value
                    .split(',')
                    .filter_map(|m| m.trim().parse::<u32>().ok())
                    .collect();
            }
            "BYSETPOS" => {
                constraints.by_set_pos = value.trim().parse::<i32>().ok();
            }
            _ => {}
        }
    }
    constraints
}

/// Map an RRULE `BYDAY` two-letter code onto a [`Weekday`]; unknown values
/// map to `None`.
fn weekday_from_rrule(code: &str) -> Option<Weekday> {
    match code.to_ascii_uppercase().as_str() {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

fn parse_base_datetime(value: &str) -> Option<DateTime<Utc>> {
    // Try full ISO-8601 datetime first.
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc));
    }
    // Fallback: date-only (treat as midnight UTC).
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Utc
            .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
            .single();
    }
    None
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_occurrence_none_returns_base() {
        let base = Utc.with_ymd_and_hms(1990, 5, 15, 0, 0, 0).unwrap();
        let from = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::None, 1, None, from, None);
        assert_eq!(result, Some(base));
    }

    #[test]
    fn next_occurrence_yearly_same_year() {
        let from = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::Yearly, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_next_year() {
        let from = Utc.with_ymd_and_hms(2024, 8, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::Yearly, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2025, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_leap_year_feb29_to_mar1() {
        let from = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_leap_year_feb29_keeps_feb29() {
        let from = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_same_month() {
        let from = Utc.with_ymd_and_hms(2024, 3, 10, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 3, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_next_month() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 4, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_next_week() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap(); // Wednesday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, None, from, None); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_tuesday_to_wednesday() {
        // from = Tuesday, base = Wednesday → next Wednesday is +1 day
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, None, from, None); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_thursday_to_wednesday() {
        // from = Thursday, base = Wednesday → next Wednesday is +6 days
        let from = Utc.with_ymd_and_hms(2024, 3, 21, 12, 0, 0).unwrap(); // Thursday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, None, from, None); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_before_base_returns_base() {
        // from predates the series start: the first occurrence is the base
        // date itself, never a date before it (negative week math must not
        // leak a pre-series candidate).
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 2, None, from, None);
        let expected = Utc.with_ymd_and_hms(2020, 3, 18, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_same_day_time_already_passed() {
        // from = Wednesday afternoon, base = Wednesday morning → next week
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 14, 0, 0).unwrap(); // Wednesday 14:00
        let result = next_occurrence(
            "2020-03-18T08:00:00Z",
            RecurrenceType::Weekly,
            1,
            None,
            from,
            None,
        ); // Wednesday 08:00
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 8, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_daily_next_day() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Daily, 1, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_fortnightly_skips_off_weeks() {
        // Base Wednesday 2020-03-18; every 2 weeks. From the on-week
        // Wednesday (2024-03-27) after its 00:00 occurrence has passed, the
        // next occurrence is the on-week Wednesday two weeks later — the
        // intervening week is an off week.
        let from = Utc.with_ymd_and_hms(2024, 3, 27, 12, 0, 0).unwrap(); // Wednesday
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 2, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 4, 10, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_fortnightly_from_off_weekday() {
        // From a Tuesday in an on-week (2024-03-19) → the next on-week
        // Wednesday (2024-03-27); the Wednesday in between is an off week.
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 2, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_every_other_month() {
        // Base 2020-03-15; every 2 months. From March 2024 → May 2024.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, 2, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_daily_every_three_days() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Daily, 3, None, from, None);
        let expected = Utc.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_until_stops_advancing_after_series_end() {
        // Weekly series ending 2024-03-25: the next occurrence (2024-03-27)
        // lies past the bound, so the scan must stop advancing.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2024, 3, 25, 0, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Weekly,
            1,
            Some(until),
            from,
            None,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn next_occurrence_until_keeps_occurrences_on_or_before_end() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2024, 3, 27, 23, 59, 59).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Weekly,
            1,
            Some(until),
            from,
            None,
        );
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_byday_multi_day() {
        // Monday-and-Wednesday series: from a Tuesday the next occurrence is
        // the same week's Wednesday, not the base weekday.
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Weekly,
            1,
            None,
            from,
            Some("FREQ=WEEKLY;BYDAY=MO,WE"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_byday_skips_passed_weekday() {
        // Wednesday 00:00 has passed by Wednesday noon: the next occurrence
        // is the following Monday, the next day in the BYDAY set.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap(); // Wednesday
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Weekly,
            1,
            None,
            from,
            Some("FREQ=WEEKLY;BYDAY=MO,WE"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 3, 25, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_byday_fortnightly_skips_off_weeks() {
        // Every 2 weeks on Monday/Wednesday: from a Tuesday in an off-week,
        // the same week's Wednesday is an off-week day, so the next
        // occurrence is the on-week Monday that starts the next valid week.
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Weekly,
            2,
            None,
            from,
            Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 3, 25, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_byday_until_stops_series() {
        // The next BYDAY occurrence (Monday 2024-03-25) lies past the bound:
        // the series has ended.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2024, 3, 24, 0, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Weekly,
            1,
            Some(until),
            from,
            Some("FREQ=WEEKLY;BYDAY=MO,WE"),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn next_occurrence_monthly_relative_second_tuesday() {
        // Second Tuesday of the month: the March occurrence (2024-03-12) has
        // passed, so the next one is April's second Tuesday.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Monthly,
            1,
            None,
            from,
            Some("FREQ=MONTHLY;BYSETPOS=2;BYDAY=TU"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 4, 9, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_relative_last_friday() {
        // Last Friday of the month: still ahead of `from` in March 2024.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Monthly,
            1,
            None,
            from,
            Some("FREQ=MONTHLY;BYSETPOS=-1;BYDAY=FR"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 3, 29, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_absolute_by_month_day() {
        // Absolute monthly on the 15th: the March occurrence has passed, so
        // the next one is April 15th.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Monthly,
            1,
            None,
            from,
            Some("FREQ=MONTHLY;BYMONTHDAY=15"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 4, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_relative_last_friday_of_june() {
        // Last Friday of June: 2024-06-28 is still ahead of `from`.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Yearly,
            1,
            None,
            from,
            Some("FREQ=YEARLY;BYMONTH=6;BYDAY=FR;BYSETPOS=-1"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 6, 28, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_absolute_month_and_day() {
        // Absolute yearly on June 15th: 2024-06-15 is still ahead of `from`.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence(
            "2020-03-18",
            RecurrenceType::Yearly,
            1,
            None,
            from,
            Some("FREQ=YEARLY;BYMONTH=6;BYMONTHDAY=15"),
        );
        let expected = Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn is_leap_year_standard_rules() {
        assert!(is_leap_year(2000)); // divisible by 400
        assert!(is_leap_year(2024)); // divisible by 4, not 100
        assert!(!is_leap_year(1900)); // divisible by 100, not 400
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn days_in_month_known_values() {
        assert_eq!(days_in_month(2024, 2), 29); // leap
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2024, 1), 31);
        assert_eq!(days_in_month(2024, 4), 30);
        assert_eq!(days_in_month(2024, 12), 31);
    }

    #[test]
    fn days_in_month_invalid_returns_zero() {
        assert_eq!(days_in_month(2024, 0), 0);
        assert_eq!(days_in_month(2024, 13), 0);
    }

    #[test]
    fn parse_base_datetime_rfc3339() {
        let dt = parse_base_datetime("2020-06-15T10:30:00Z").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2020-06-15 10:30");
    }

    #[test]
    fn parse_base_datetime_date_only_is_midnight_utc() {
        let dt = parse_base_datetime("2020-06-15").unwrap();
        assert_eq!(dt.format("%H:%M:%S").to_string(), "00:00:00");
    }

    #[test]
    fn parse_base_datetime_invalid_returns_none() {
        assert!(parse_base_datetime("not a date").is_none());
        assert!(parse_base_datetime("").is_none());
        assert!(parse_base_datetime("2020/06/15").is_none());
    }

    #[test]
    fn parse_base_datetime_with_offset_normalises_to_utc() {
        // +02:00 offset → 08:00 local becomes 06:00 UTC.
        let dt = parse_base_datetime("2020-06-15T08:00:00+02:00").unwrap();
        assert_eq!(dt.format("%H:%M").to_string(), "06:00");
    }
}
