# Job Queue & Background Scheduler

## Overview

`mimir-core::job_queue` provides a durable, SQLite-backed async job queue for background tasks. `mimir-core::scheduler` adds a unified **BackgroundScheduler** that wraps the queue with deduplication, debounce, and user-downtime gating.

All scheduled background jobs — nightly knowledge graph optimization and any future scheduled work — follow the same lifecycle rules (memory condensation moved to the hooks engine in #386, where each hook applies its own queue policy):

1. **Deduplication** — Submitting the same job type twice only adds it once to the pending set.
2. **Debounce** — Rapid successive submissions reset a timer (default 5 s). The job is only considered ready after the timer elapses.
3. **Cooldown** — After the debounce, the scheduler waits until no user activity has occurred for a cooldown period (default 60 s).
4. **Idle gate** — The scheduler also checks that the LLM worker pool is completely idle (no queued or in-flight jobs) before dispatching.
5. **No automatic user-triggered cancellation** — A user message does not automatically cancel a running job; explicit `JobQueue::cancel(job_id)` calls and daemon shutdown are the cancellation paths.
6. **Scheduled-job poll** — Every 60 s the scheduler checks the durable queue for jobs whose `next_run_at` has passed and submits them through the same flow.

## Cancellation & resource limits (issue #91)

Each run gets a `tokio_util::sync::CancellationToken` exposed through `JobContext::cancellation_token()`. Cooperative handlers `tokio::select!` on `ctx.cancellation_token().cancelled()` at checkpoint boundaries so long-running passes can persist state and exit cleanly. Cancellation is best-effort: the token only signals the run, and the dedicated run thread is neither aborted nor joined, so synchronous or blocking work can keep running until it finishes. `JobQueue::cancel(job_id)` cancels one running job, `JobQueue::cancel_all()` cancels every running job (called by `BackgroundScheduler::shutdown()` so daemon shutdown never waits for a long job), and a cancelled run is recorded as `JobRunStatus::Cancelled` in `job_runs` even when the handler returns `Ok(())` after observing the token.

Jobs may declare best-effort `JobResourceLimits` via `Job::with_resource_limits(...)`: `cpu_cores` (Linux CPU affinity), `nice_level` (POSIX scheduling priority), and `memory_limit_bytes` (Linux cgroup v2 `memory.max`). Enforcement is OS-specific and never fails the job — unsupported platforms, missing permissions, or unwritable cgroup filesystems degrade to a debug log and the job runs without the limit. Each run executes on a fresh dedicated thread (named `mimir-job-<id>`) so thread-local limits (affinity, nice) are discarded when the thread exits and never leak into pooled threads; the process-wide cgroup move is restored on drop. The cgroup memory cap is process-wide: the whole daemon is moved into the job cgroup for the run, so `memory_limit_bytes` caps the entire process while the job executes.

## Typed Job Identifiers

The scheduler's `DaemonJob` enum identifies the daemon-scheduled jobs:

```rust
pub enum DaemonJob {
    KnowledgeOptimization,
}
```

`DaemonJob::job_id()` maps each variant to its persistent string ID (`knowledge.optimization`). Other background jobs — `knowledge.pending_cleanup` and `events.upcoming_scan_{idx}` — are registered directly as plain `Job` entries. `JobQueue::run_now` and `JobQueue::status` accept the persistent job ID as `&str`.

## Public API

### JobQueue

- `JobQueue::init(path)` — create or open the queue database.
- `JobQueue::register(job)` — persist a job definition and store its handler.
- `JobQueue::run_now(job_id)` — execute a job immediately, recording the run.
- `JobQueue::cancel(job_id)` — request cancellation of a running job (returns whether one was found).
- `JobQueue::cancel_all()` — request cancellation of every running job (daemon shutdown).
- `JobQueue::is_running(job_id)` — whether the job has an in-flight run.
- `JobQueue::status(job_id)` — get schedule and last run for a job.
- `JobQueue::list_jobs()` — list all registered jobs with status.
- `Job::with_resource_limits(limits)` — attach best-effort CPU/nice/memory limits to a job.
- `JobContext::cancellation_token()` / `JobContext::is_cancelled()` — observe cancellation from inside a handler.

### BackgroundScheduler

- `BackgroundScheduler::new(job_queue, llm, debounce, cooldown)` — create scheduler and shutdown receiver.
- `scheduler.submit(job).await` — queue a job for deduped, debounced, cooldown-gated dispatch. Also deduplicates against the job currently running.
- `scheduler.force_submit(job)` — bypass all gates and run immediately through `JobQueue`.
- `scheduler.notify_user_activity()` — reset the cooldown timer.
- `scheduler.start(shutdown_rx)` — spawn the dispatch loop.
- `scheduler.shutdown()` — signal the dispatch loop to exit gracefully.

## `DailySchedule`

A daily local-time schedule (`HH:MM`) stored as a `chrono::NaiveTime` and converted to UTC for daemon state. Key points:

- `DailySchedule::parse("HH:MM")` enforces a **strict** five-character `HH:MM` format with zero-padded two-digit fields. Non-zero-padded inputs such as `"2:30"` or `"9:5"` are rejected with `JobError::InvalidSchedule`, even though chrono's `%H` parser is padding-agnostic. This keeps user-authored `[[scheduler]]`/job schedule strings deterministic (issue #162).
- `DailySchedule::next_after(now)` returns the next UTC instant strictly after `now`. Conversion from local wall-clock to UTC uses `DailySchedule::naive_to_utc_local`, which resolves DST gaps (spring-forward) and ambiguities (falls back to the earlier offset).
- `naive_to_utc_local` is `pub` and shared with the CLI date filters (`mimir/src/kb/mod.rs::parse_datetime`) so local times are interpreted consistently across the daemon and CLI (issue #168).

`JobError::is_not_registered` and `JobError::is_already_running` are documented predicate helpers over the error enum (issue #161).

## Wire strings

The HTTP API (`mimir-api-types`) carries job priorities and run statuses as lowercase strings. `JobPriority::as_str()` and `JobRunStatus::as_str()` are the single source of truth for those wire values (issue #264) — independent of the derived `Debug` repr. Note `JobRunStatus::TimedOut` maps to `"timed_out"` (underscored), matching the DB representation rather than the `Debug`-derived `"timedout"`; `JobRunStatus::from_str` parses the same strings, so the two directions stay symmetric.

## Configuration

```toml
[scheduler]
debounce_seconds = 5
cooldown_seconds = 60

[knowledge.optimization]
cpu_cores = 1
nice_level = 10
timeout_minutes = 120
schedule_time = "02:00"
# memory_limit_mb = 2048  # Optional: best-effort cgroup v2 memory cap (MiB)
```

## Integration

The daemon initialises the scheduler in `AppState::from_config_with_llm`, registers the `knowledge.optimization` job in the durable `JobQueue` with the configured `cpu_cores`/`nice_level`/`memory_limit_mb` resource limits, and starts the dispatch loop in `start_server_with_llm_and_listener`. Memory condensation is now a hook (issue #386): the KG dirty notify path triggers `Trigger::FactInserted`, and the `memory.condensation` hook (global `SingularLastWins`, idle-gated) runs the condenser through the hooks engine's dispatch loop.
