//! Recurrence resolution helpers shared by the events subsystem.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};

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
/// indefinitely).
pub fn next_occurrence(
    date_value: &str,
    recurrence: RecurrenceType,
    interval: i32,
    until: Option<DateTime<Utc>>,
    from: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let base = parse_base_datetime(date_value)?;
    let interval = interval.max(1) as i64;

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
            if from.date_naive() < base.date_naive() {
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
                let mut candidate = month_candidate(base, k * interval)?;
                if candidate < from {
                    candidate = month_candidate(base, (k + 1) * interval)?;
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
                let mut candidate = year_candidate(base, k * interval)?;
                if candidate < from {
                    candidate = year_candidate(base, (k + 1) * interval)?;
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

/// The base day (clamped to the month length) in the month `months_after`
/// months after the base date.
fn month_candidate(base: DateTime<Utc>, months_after: i64) -> Option<DateTime<Utc>> {
    let base_date = base.date_naive();
    let total = base_date.year() as i64 * 12 + (base_date.month() as i64 - 1) + months_after;
    let year = (total / 12) as i32;
    let month = (total % 12) as u32 + 1;
    let day = std::cmp::min(base_date.day(), days_in_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| Utc.from_local_datetime(&d.and_time(base.time())).single())
}

/// The base month/day in the year `years_after` years after the base date;
/// Feb 29 falls back to Mar 1 in non-leap years.
fn year_candidate(base: DateTime<Utc>, years_after: i64) -> Option<DateTime<Utc>> {
    let base_date = base.date_naive();
    let year = base_date.year() + years_after as i32;
    let (month, day) = if base_date.month() == 2 && base_date.day() == 29 && !is_leap_year(year) {
        (3, 1)
    } else {
        (base_date.month(), base_date.day())
    };
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| Utc.from_local_datetime(&d.and_time(base.time())).single())
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
        let result = next_occurrence("1990-05-15", RecurrenceType::None, 1, None, from);
        assert_eq!(result, Some(base));
    }

    #[test]
    fn next_occurrence_yearly_same_year() {
        let from = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::Yearly, 1, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_next_year() {
        let from = Utc.with_ymd_and_hms(2024, 8, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::Yearly, 1, None, from);
        let expected = Utc.with_ymd_and_hms(2025, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_leap_year_feb29_to_mar1() {
        let from = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, 1, None, from);
        let expected = Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_leap_year_feb29_keeps_feb29() {
        let from = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, 1, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_same_month() {
        let from = Utc.with_ymd_and_hms(2024, 3, 10, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, 1, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 3, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_next_month() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, 1, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 4, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_next_week() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap(); // Wednesday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, None, from); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_tuesday_to_wednesday() {
        // from = Tuesday, base = Wednesday → next Wednesday is +1 day
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, None, from); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_thursday_to_wednesday() {
        // from = Thursday, base = Wednesday → next Wednesday is +6 days
        let from = Utc.with_ymd_and_hms(2024, 3, 21, 12, 0, 0).unwrap(); // Thursday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, None, from); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_before_base_returns_base() {
        // from predates the series start: the first occurrence is the base
        // date itself, never a date before it (negative week math must not
        // leak a pre-series candidate).
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 2, None, from);
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
        ); // Wednesday 08:00
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 8, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_daily_next_day() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Daily, 1, None, from);
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
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 2, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 4, 10, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_fortnightly_from_off_weekday() {
        // From a Tuesday in an on-week (2024-03-19) → the next on-week
        // Wednesday (2024-03-27); the Wednesday in between is an off week.
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 2, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_every_other_month() {
        // Base 2020-03-15; every 2 months. From March 2024 → May 2024.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, 2, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_daily_every_three_days() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Daily, 3, None, from);
        let expected = Utc.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_until_stops_advancing_after_series_end() {
        // Weekly series ending 2024-03-25: the next occurrence (2024-03-27)
        // lies past the bound, so the scan must stop advancing.
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2024, 3, 25, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, Some(until), from);
        assert_eq!(result, None);
    }

    #[test]
    fn next_occurrence_until_keeps_occurrences_on_or_before_end() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2024, 3, 27, 23, 59, 59).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, 1, Some(until), from);
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
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
