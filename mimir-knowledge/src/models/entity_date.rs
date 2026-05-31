//! Entity date model and recurrence resolution.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::models::enums::RecurrenceType;

/// A date (or recurring date) associated with an entity.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct EntityDate {
    pub id: i32,
    pub entity_id: i32,
    pub date_type_id: i16,
    pub date_value: String,
    pub recurrence_type_id: i16,
    pub custom_label: Option<String>,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
}

/// Compute the next occurrence of a recurring date on or after `from`.
///
/// - `None` → returns the original `date_value` parsed as a one-time date.
/// - `Yearly` → next anniversary; Feb 29 falls back to Mar 1 in non-leap years.
/// - `Monthly` → same day each month; if day exceeds month length, falls back to last day.
/// - `Weekly` → same weekday each week.
/// - `Daily` → every day (returns `from` truncated to midnight if `date_value` is date-only,
///   or `from` if datetime).
pub fn next_occurrence(
    date_value: &str,
    recurrence: RecurrenceType,
    from: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let base = parse_base_datetime(date_value)?;

    match recurrence {
        RecurrenceType::None => Some(base),
        RecurrenceType::Daily => {
            let base_date = base.date_naive();
            let from_date = from.date_naive();
            if base_date > from_date {
                Some(base)
            } else {
                let days_ahead = (from_date - base_date).num_days();
                let candidate = base + Duration::days(days_ahead);
                if candidate >= from {
                    Some(candidate)
                } else {
                    Some(candidate + Duration::days(1))
                }
            }
        }
        RecurrenceType::Weekly => {
            let from_weekday = from.date_naive().weekday().num_days_from_monday() as i64;
            let base_weekday = base.date_naive().weekday().num_days_from_monday() as i64;
            let mut days_ahead = (base_weekday - from_weekday + 7) % 7;
            if days_ahead == 0 && base.time() < from.time() {
                days_ahead = 7;
            }
            Some(from.date_naive() + Duration::days(days_ahead))
                .and_then(|d| Utc.from_local_datetime(&d.and_time(base.time())).single())
        }
        RecurrenceType::Monthly => {
            let base_date = base.date_naive();
            let mut candidate_year = from.year();
            let mut candidate_month = from.month();
            let candidate_day = std::cmp::min(
                base_date.day(),
                days_in_month(candidate_year, candidate_month),
            );
            let mut candidate =
                NaiveDate::from_ymd_opt(candidate_year, candidate_month, candidate_day)?
                    .and_time(base.time());
            if candidate < from.naive_utc() {
                candidate_month += 1;
                if candidate_month > 12 {
                    candidate_month = 1;
                    candidate_year += 1;
                }
                let next_day = std::cmp::min(
                    base_date.day(),
                    days_in_month(candidate_year, candidate_month),
                );
                candidate = NaiveDate::from_ymd_opt(candidate_year, candidate_month, next_day)?
                    .and_time(base.time());
            }
            Utc.from_local_datetime(&candidate).single()
        }
        RecurrenceType::Yearly => {
            let base_date = base.date_naive();
            let candidate_year = from.year();
            let mut candidate_day = base_date.day();
            // Feb 29 fallback to Mar 1 in non-leap years.
            if base_date.month() == 2 && candidate_day == 29 && !is_leap_year(candidate_year) {
                candidate_day = 1;
                let candidate = NaiveDate::from_ymd_opt(candidate_year, 3, candidate_day)?
                    .and_time(base.time());
                if candidate < from.naive_utc() {
                    let next_year = candidate_year + 1;
                    let next_day = if is_leap_year(next_year) { 29 } else { 1 };
                    let next_month = if is_leap_year(next_year) { 2 } else { 3 };
                    return Utc
                        .from_local_datetime(
                            &NaiveDate::from_ymd_opt(next_year, next_month, next_day)?
                                .and_time(base.time()),
                        )
                        .single();
                }
                return Utc.from_local_datetime(&candidate).single();
            }
            let mut candidate =
                NaiveDate::from_ymd_opt(candidate_year, base_date.month(), candidate_day)?
                    .and_time(base.time());
            if candidate < from.naive_utc() {
                let next_year = candidate_year + 1;
                let next_day = if base_date.month() == 2
                    && base_date.day() == 29
                    && !is_leap_year(next_year)
                {
                    1
                } else {
                    base_date.day()
                };
                let next_month = if base_date.month() == 2 && base_date.day() == 29 && next_day == 1
                {
                    3
                } else {
                    base_date.month()
                };
                candidate =
                    NaiveDate::from_ymd_opt(next_year, next_month, next_day)?.and_time(base.time());
            }
            Utc.from_local_datetime(&candidate).single()
        }
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
        let result = next_occurrence("1990-05-15", RecurrenceType::None, from);
        assert_eq!(result, Some(base));
    }

    #[test]
    fn next_occurrence_yearly_same_year() {
        let from = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::Yearly, from);
        let expected = Utc.with_ymd_and_hms(2024, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_next_year() {
        let from = Utc.with_ymd_and_hms(2024, 8, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("1990-05-15", RecurrenceType::Yearly, from);
        let expected = Utc.with_ymd_and_hms(2025, 5, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_leap_year_feb29_to_mar1() {
        let from = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, from);
        let expected = Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_yearly_leap_year_feb29_keeps_feb29() {
        let from = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let result = next_occurrence("2000-02-29", RecurrenceType::Yearly, from);
        let expected = Utc.with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_same_month() {
        let from = Utc.with_ymd_and_hms(2024, 3, 10, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, from);
        let expected = Utc.with_ymd_and_hms(2024, 3, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_monthly_next_month() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        let result = next_occurrence("2020-03-15", RecurrenceType::Monthly, from);
        let expected = Utc.with_ymd_and_hms(2024, 4, 15, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_next_week() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap(); // Wednesday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, from); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_tuesday_to_wednesday() {
        // from = Tuesday, base = Wednesday → next Wednesday is +1 day
        let from = Utc.with_ymd_and_hms(2024, 3, 19, 12, 0, 0).unwrap(); // Tuesday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, from); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_from_thursday_to_wednesday() {
        // from = Thursday, base = Wednesday → next Wednesday is +6 days
        let from = Utc.with_ymd_and_hms(2024, 3, 21, 12, 0, 0).unwrap(); // Thursday noon
        let result = next_occurrence("2020-03-18", RecurrenceType::Weekly, from); // Wednesday base
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_weekly_same_day_time_already_passed() {
        // from = Wednesday afternoon, base = Wednesday morning → next week
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 14, 0, 0).unwrap(); // Wednesday 14:00
        let result = next_occurrence("2020-03-18T08:00:00Z", RecurrenceType::Weekly, from); // Wednesday 08:00
        let expected = Utc.with_ymd_and_hms(2024, 3, 27, 8, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn next_occurrence_daily_next_day() {
        let from = Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap();
        let result = next_occurrence("2020-03-18", RecurrenceType::Daily, from);
        let expected = Utc.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap();
        assert_eq!(result, Some(expected));
    }
}
