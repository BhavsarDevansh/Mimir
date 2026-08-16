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
async fn job_queue_cancels_running_job() {
    let dir = tempfile::tempdir().unwrap();
    let queue = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();

    queue
        .register(Job::new(
            "test.cancel",
            JobPriority::System,
            None,
            false,
            |_ctx| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(())
                })
            },
        ))
        .await
        .unwrap();

    let run_task = tokio::spawn({
        let queue = queue.clone();
        async move { queue.run_now("test.cancel").await.unwrap() }
    });

    // Wait until the job is actually running before cancelling it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !queue.is_running("test.cancel").await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(queue.is_running("test.cancel").await);

    assert!(queue.cancel("test.cancel"));
    let run = run_task.await.unwrap();

    assert_eq!(run.status, JobRunStatus::Cancelled);
    assert!(run.finished_at.unwrap() >= run.started_at);
}

#[tokio::test]
async fn job_queue_graceful_cancellation_records_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let queue = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();

    queue
        .register(Job::new(
            "test.graceful",
            JobPriority::System,
            None,
            false,
            |ctx: JobContext| {
                Box::pin(async move {
                    let cancelled = ctx.cancellation_token();
                    tokio::select! {
                        _ = cancelled.cancelled() => Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(30)) => Ok(()),
                    }
                })
            },
        ))
        .await
        .unwrap();

    let run_task = tokio::spawn({
        let queue = queue.clone();
        async move { queue.run_now("test.graceful").await.unwrap() }
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !queue.is_running("test.graceful").await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(queue.is_running("test.graceful").await);

    assert!(queue.cancel("test.graceful"));
    let run = run_task.await.unwrap();

    // The handler returned Ok(()) after observing the token, but the run must
    // still be recorded as cancelled.
    assert_eq!(run.status, JobRunStatus::Cancelled);
}

#[tokio::test]
async fn job_queue_cancel_all_cancels_running_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let queue = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();

    for id in ["test.cancel_a", "test.cancel_b"] {
        queue
            .register(Job::new(id, JobPriority::System, None, false, |_ctx| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(())
                })
            }))
            .await
            .unwrap();
    }

    let run_a = tokio::spawn({
        let queue = queue.clone();
        async move { queue.run_now("test.cancel_a").await.unwrap() }
    });
    let run_b = tokio::spawn({
        let queue = queue.clone();
        async move { queue.run_now("test.cancel_b").await.unwrap() }
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(queue.is_running("test.cancel_a").await && queue.is_running("test.cancel_b").await)
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(queue.is_running("test.cancel_a").await);
    assert!(queue.is_running("test.cancel_b").await);

    queue.cancel_all();
    assert_eq!(run_a.await.unwrap().status, JobRunStatus::Cancelled);
    assert_eq!(run_b.await.unwrap().status, JobRunStatus::Cancelled);
}

#[tokio::test]
async fn job_queue_cancelled_status_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("jobs.db");
    let queue = JobQueue::init(&db_path).await.unwrap();

    queue
        .register(Job::new(
            "test.persist",
            JobPriority::System,
            None,
            false,
            |_ctx| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(())
                })
            },
        ))
        .await
        .unwrap();

    let run_task = tokio::spawn({
        let queue = queue.clone();
        async move { queue.run_now("test.persist").await.unwrap() }
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !queue.is_running("test.persist").await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(queue.is_running("test.persist").await);

    assert!(queue.cancel("test.persist"));
    assert_eq!(run_task.await.unwrap().status, JobRunStatus::Cancelled);

    drop(queue);
    let reopened = JobQueue::init(&db_path).await.unwrap();
    let status = reopened.status("test.persist").await.unwrap();
    assert_eq!(
        status.last_run.as_ref().unwrap().status,
        JobRunStatus::Cancelled
    );
}

#[tokio::test]
async fn job_queue_records_panicking_handler_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let queue = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();

    queue
        .register(Job::new(
            "test.panic",
            JobPriority::System,
            None,
            false,
            |_ctx| {
                Box::pin(async move {
                    panic!("boom");
                })
            },
        ))
        .await
        .unwrap();

    let run = queue.run_now("test.panic").await.unwrap();
    assert_eq!(run.status, JobRunStatus::Failed);
    let error = run.error.expect("failed run records an error");
    assert!(error.contains("panicked"), "unexpected error: {error}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn job_queue_resource_limits_apply_during_run() {
    use mimir_core::job_queue::JobResourceLimits;
    use nix::sched::{CpuSet, sched_getaffinity};
    use nix::unistd::Pid;
    use rustix::process::getpriority_process;

    // Lowering the nice value needs privileges; skip when the target is more
    // urgent than the current value.
    if getpriority_process(None).unwrap_or(0) > 10 {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let queue = JobQueue::init(dir.path().join("jobs.db")).await.unwrap();

    let limits = JobResourceLimits {
        cpu_cores: Some(1),
        nice_level: Some(10),
        memory_limit_bytes: None,
    };
    queue
        .register(
            Job::new("test.limits", JobPriority::System, None, false, |_ctx| {
                Box::pin(async move {
                    let set = sched_getaffinity(Pid::from_raw(0)).unwrap();
                    let mut count = 0usize;
                    for cpu in 0..CpuSet::count() {
                        if set.is_set(cpu).unwrap_or(false) {
                            count += 1;
                        }
                    }
                    assert_eq!(count, 1);
                    assert_eq!(getpriority_process(None).unwrap(), 10);
                    Ok(())
                })
            })
            .with_resource_limits(limits),
        )
        .await
        .unwrap();

    let run = queue.run_now("test.limits").await.unwrap();
    assert_eq!(run.status, JobRunStatus::Succeeded);
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
