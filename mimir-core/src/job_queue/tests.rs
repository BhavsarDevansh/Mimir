//! Job queue unit tests.

use super::*;

#[test]
fn job_priority_from_i16_known_values() {
    assert_eq!(JobPriority::from_i16(0), Some(JobPriority::System));
    assert_eq!(JobPriority::from_i16(1), Some(JobPriority::Maintenance));
    assert_eq!(JobPriority::from_i16(2), Some(JobPriority::User));
}

#[test]
fn job_priority_from_i16_rejects_unknown() {
    assert_eq!(JobPriority::from_i16(-1), None);
    assert_eq!(JobPriority::from_i16(3), None);
    assert_eq!(JobPriority::from_i16(i16::MIN), None);
    assert_eq!(JobPriority::from_i16(i16::MAX), None);
}

#[test]
fn job_priority_as_str_matches_wire_contract() {
    assert_eq!(JobPriority::System.as_str(), "system");
    assert_eq!(JobPriority::Maintenance.as_str(), "maintenance");
    assert_eq!(JobPriority::User.as_str(), "user");
}

#[test]
fn job_run_status_roundtrips_through_str() {
    for status in [
        JobRunStatus::Running,
        JobRunStatus::Succeeded,
        JobRunStatus::Failed,
        JobRunStatus::TimedOut,
        JobRunStatus::Cancelled,
    ] {
        assert_eq!(JobRunStatus::from_str(status.as_str()), status);
    }
}

#[test]
fn job_run_status_as_str_matches_wire_contract() {
    assert_eq!(JobRunStatus::Running.as_str(), "running");
    assert_eq!(JobRunStatus::Succeeded.as_str(), "succeeded");
    assert_eq!(JobRunStatus::Failed.as_str(), "failed");
    assert_eq!(JobRunStatus::TimedOut.as_str(), "timed_out");
    assert_eq!(JobRunStatus::Cancelled.as_str(), "cancelled");
}

#[test]
fn job_run_status_from_str_unknown_falls_back_to_failed() {
    assert_eq!(JobRunStatus::from_str("nope"), JobRunStatus::Failed);
    assert_eq!(JobRunStatus::from_str(""), JobRunStatus::Failed);
}

#[test]
fn daily_schedule_parse_valid() {
    let s = DailySchedule::parse("02:30").unwrap();
    assert_eq!(s.as_hhmm(), "02:30");
}

#[test]
fn daily_schedule_parse_rejects_invalid_formats() {
    // Out-of-range hour/minute are rejected by chrono's %H:%M parser.
    assert!(matches!(
        DailySchedule::parse("25:00"),
        Err(JobError::InvalidSchedule(_))
    ));
    assert!(matches!(
        DailySchedule::parse("abc"),
        Err(JobError::InvalidSchedule(_))
    ));
    assert!(matches!(
        DailySchedule::parse(""),
        Err(JobError::InvalidSchedule(_))
    ));
    assert!(matches!(
        DailySchedule::parse("10:99"),
        Err(JobError::InvalidSchedule(_))
    ));
    // Non-zero-padded forms are rejected by the strict HH:MM guard.
    assert!(matches!(
        DailySchedule::parse("2:30"),
        Err(JobError::InvalidSchedule(_))
    ));
    assert!(matches!(
        DailySchedule::parse("9:5"),
        Err(JobError::InvalidSchedule(_))
    ));
    // Wrong separator / missing field.
    assert!(matches!(
        DailySchedule::parse("02-30"),
        Err(JobError::InvalidSchedule(_))
    ));
    assert!(matches!(
        DailySchedule::parse("2:30:00"),
        Err(JobError::InvalidSchedule(_))
    ));
}

#[test]
fn daily_schedule_parse_rejects_non_zero_padded_input() {
    // The strict `HH:MM` guard rejects single-digit hour/minute fields
    // that chrono's padding-agnostic `%H` parser would otherwise accept.
    assert!(matches!(
        DailySchedule::parse("2:30"),
        Err(JobError::InvalidSchedule(_))
    ));
    assert!(matches!(
        DailySchedule::parse("9:5"),
        Err(JobError::InvalidSchedule(_))
    ));
}

#[test]
fn daily_schedule_as_hhmm_zero_pads() {
    let s = DailySchedule::new(NaiveTime::from_hms_opt(9, 5, 0).unwrap());
    assert_eq!(s.as_hhmm(), "09:05");
}

#[test]
fn daily_schedule_next_after_today_reschedules_for_tomorrow() {
    // 03:00 local. With `now` late in the day, the next run is tomorrow.
    let s = DailySchedule::new(NaiveTime::from_hms_opt(3, 0, 0).unwrap());
    let now = Utc::now();
    let next = s.next_after(now);
    assert!(next > now);
    // Within the next 25 hours (allows for DST shifts).
    assert!(next - now < chrono::Duration::hours(25));
    // The local wall-clock time of the next run must match the schedule.
    let local_next = chrono::Local.from_utc_datetime(&next.naive_utc());
    assert_eq!(local_next.time(), s.time);
}

#[test]
fn daily_schedule_next_after_future_today_still_ahead() {
    // Use a time far in the future relative to "now" so today's slot is ahead.
    let far_future_hour: u32 = 23;
    let s = DailySchedule::new(NaiveTime::from_hms_opt(far_future_hour, 59, 0).unwrap());
    let now = Utc::now();
    let next = s.next_after(now);
    assert!(next > now);
    assert!(next - now <= chrono::Duration::hours(25));
    let local_next = chrono::Local.from_utc_datetime(&next.naive_utc());
    assert_eq!(local_next.time(), s.time);
}

#[test]
fn job_error_predicates() {
    assert!(JobError::JobNotRegistered("x".to_string()).is_not_registered());
    assert!(!JobError::Database(sqlx::Error::PoolClosed).is_not_registered());

    assert!(JobError::JobAlreadyRunning("y".to_string()).is_already_running());
    assert!(!JobError::JobNotRegistered("z".to_string()).is_already_running());
}

#[test]
fn job_context_exposes_job_id_and_cancellation() {
    let token = tokio_util::sync::CancellationToken::new();
    let ctx = JobContext::new("daily.cleanup".to_string(), token.clone());
    assert_eq!(ctx.job_id(), "daily.cleanup");
    assert!(!ctx.is_cancelled());
    token.cancel();
    assert!(ctx.is_cancelled());
}

#[test]
fn job_resource_limits_default_to_none() {
    let limits = JobResourceLimits::default();
    assert_eq!(limits.cpu_cores, None);
    assert_eq!(limits.nice_level, None);
    assert_eq!(limits.memory_limit_bytes, None);
}

#[test]
fn job_with_resource_limits_exposes_them() {
    let limits = JobResourceLimits {
        cpu_cores: Some(2),
        nice_level: Some(10),
        memory_limit_bytes: Some(512 * 1024 * 1024),
    };
    let job = Job::new("test.limits", JobPriority::System, None, false, |_ctx| {
        Box::pin(async { Ok(()) })
    })
    .with_resource_limits(limits);
    assert_eq!(job.resource_limits(), limits);
}

#[test]
fn daily_schedule_serializes_roundtrip() {
    let s = DailySchedule::new(NaiveTime::from_hms_opt(23, 59, 0).unwrap());
    let json = serde_json::to_string(&s).unwrap();
    let back: DailySchedule = serde_json::from_str(&json).unwrap();
    assert_eq!(back.as_hhmm(), "23:59");
}

#[test]
fn job_priority_serializes_as_variant_name() {
    // serde serializes C-like enums as their variant name string by default;
    // the DB layer casts to i16 separately.
    for p in [
        JobPriority::System,
        JobPriority::Maintenance,
        JobPriority::User,
    ] {
        let json = serde_json::to_string(&p).unwrap();
        let back: JobPriority = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
    assert_eq!(
        serde_json::to_string(&JobPriority::User).unwrap(),
        "\"User\""
    );
}

#[test]
fn job_run_status_serializes_as_variant_name() {
    for s in [
        JobRunStatus::Running,
        JobRunStatus::Succeeded,
        JobRunStatus::Failed,
        JobRunStatus::TimedOut,
        JobRunStatus::Cancelled,
    ] {
        let json = serde_json::to_string(&s).unwrap();
        let back: JobRunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
