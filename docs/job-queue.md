# Job Queue & Background Scheduler

## Overview

`mimir-core::job_queue` provides a durable, SQLite-backed async job queue for background tasks. `mimir-core::scheduler` adds a unified **BackgroundScheduler** that wraps the queue with deduplication, debounce, and user-downtime gating.

All background jobs — memory condensation, nightly knowledge graph optimization, and any future background work — follow the same lifecycle rules:

1. **Deduplication** — Submitting the same job type twice only adds it once to the pending set.
2. **Debounce** — Rapid successive submissions reset a timer (default 5 s). The job is only considered ready after the timer elapses.
3. **Cooldown** — After the debounce, the scheduler waits until no user activity has occurred for a cooldown period (default 60 s).
4. **Idle gate** — The scheduler also checks that the LLM worker pool is completely idle (no queued or in-flight jobs) before dispatching.
5. **No mid-flight cancellation** — Once a job starts, it runs to completion even if a user message arrives.
6. **Scheduled-job poll** — Every 60 s the scheduler checks the durable queue for jobs whose `next_run_at` has passed and submits them through the same flow.

## Typed Job Identifiers

Background jobs are identified by the `DaemonJob` enum instead of raw strings:

```rust
pub enum DaemonJob {
    MemoryCondensation,
    KnowledgeOptimization,
}
```

`JobQueue::run_now` and `JobQueue::status` accept `DaemonJob` for type-safe dispatch.

## Public API

### JobQueue

- `JobQueue::init(path)` — create or open the queue database.
- `JobQueue::register(job)` — persist a job definition and store its handler.
- `JobQueue::run_now(job_id)` — execute a job immediately, recording the run.
- `JobQueue::status(job_id)` — get schedule and last run for a job.
- `JobQueue::list_jobs()` — list all registered jobs with status.

### BackgroundScheduler

- `BackgroundScheduler::new(job_queue, llm, debounce, cooldown)` — create scheduler and shutdown receiver.
- `scheduler.submit(job).await` — queue a job for deduped, debounced, cooldown-gated dispatch. Also deduplicates against the job currently running.
- `scheduler.force_submit(job)` — bypass all gates and run immediately through `JobQueue`.
- `scheduler.notify_user_activity()` — reset the cooldown timer.
- `scheduler.start(shutdown_rx)` — spawn the dispatch loop.
- `scheduler.shutdown()` — signal the dispatch loop to exit gracefully.

## Configuration

```toml
[scheduler]
debounce_seconds = 5
cooldown_seconds = 60
```

## Integration

The daemon initialises the scheduler in `AppState::from_config_with_llm`, registers the `knowledge.optimization` job in the durable `JobQueue`, and starts the dispatch loop in `start_server_with_llm_and_listener`. The condensation dirty signal from `KnowledgeGraph` drives memory condensation via a `tokio::sync::Notify` listener that submits the job through the scheduler.
