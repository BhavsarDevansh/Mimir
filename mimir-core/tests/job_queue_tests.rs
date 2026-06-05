use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use chrono::{NaiveTime, Utc};
use mimir_core::job_queue::{DailySchedule, Job, JobContext, JobPriority, JobQueue, JobRunStatus};

#[tokio::test]
async fn job_queue_persists_registered_jobs_and_manual_runs() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("jobs.db");
    let queue = JobQueue::init(&db_path).await.unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let job_calls = Arc::clone(&calls);
    let job = Job::new(
        "test.job",
        JobPriority::Maintenance,
        Some(DailySchedule::new(
            NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
        )),
        true,
        move |_ctx: JobContext| {
            let job_calls = Arc::clone(&job_calls);
            Box::pin(async move {
                job_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        },
    );

    queue.register(job).await.unwrap();
    let run = queue.run_now("test.job").await.unwrap();

    assert_eq!(run.job_id, "test.job");
    assert_eq!(run.status, JobRunStatus::Succeeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    drop(queue);

    let reopened = JobQueue::init(&db_path).await.unwrap();
    let status = reopened.status("test.job").await.unwrap();

    assert_eq!(status.job_id, "test.job");
    let last_run = status.last_run.as_ref().unwrap();
    assert_eq!(last_run.status, JobRunStatus::Succeeded);
    assert_eq!(
        status.schedule.unwrap().next_after(last_run.started_at),
        DailySchedule::new(NaiveTime::from_hms_opt(2, 0, 0).unwrap())
            .next_after(last_run.started_at)
    );
}

#[tokio::test]
async fn job_queue_times_out_long_running_job() {
    let dir = tempfile::tempdir().unwrap();
    let queue = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();

    queue.set_default_timeout(Duration::from_millis(25)).await;
    queue
        .register(Job::new(
            "test.timeout",
            JobPriority::System,
            None,
            false,
            |_ctx| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(())
                })
            },
        ))
        .await
        .unwrap();

    let run = queue.run_now("test.timeout").await.unwrap();

    assert_eq!(run.status, JobRunStatus::TimedOut);
    assert!(run.finished_at.unwrap() >= run.started_at);
}

#[tokio::test]
async fn daily_schedule_next_after_is_later_than_now() {
    let schedule = DailySchedule::new(NaiveTime::from_hms_opt(3, 0, 0).unwrap());
    let now = Utc::now();
    let next = schedule.next_after(now);
    assert!(next > now);
    // Should be within the next 25 hours
    assert!(next - now < chrono::Duration::hours(25));
}

#[tokio::test]
async fn daily_schedule_handles_dst_spring_forward_without_panic() {
    // US spring forward 2026-03-08: 02:00 does not exist in most US timezones.
    let schedule = DailySchedule::new(NaiveTime::from_hms_opt(2, 30, 0).unwrap());
    let now = chrono::DateTime::parse_from_rfc3339("2026-03-07T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    // Must not panic even if the local time falls in a DST gap.
    let next = schedule.next_after(now);
    assert!(next > now);
}

#[tokio::test]
async fn daily_schedule_handles_dst_fall_back_without_panic() {
    // US fall back 2026-11-01: 02:00 occurs twice.
    let schedule = DailySchedule::new(NaiveTime::from_hms_opt(2, 30, 0).unwrap());
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let next = schedule.next_after(now);
    assert!(next > now);
}
