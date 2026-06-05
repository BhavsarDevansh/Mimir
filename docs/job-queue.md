# Job Queue

## Overview

`mimir-core::job_queue` provides a durable, SQLite-backed async job queue for background tasks. It is used by the Mimir daemon to schedule and run maintenance jobs such as the nightly knowledge graph optimization.

## Design

- **Persistence**: Job definitions and run history are stored in `jobs.db` (separate from `knowledge.db` and `context.db`).
- **Handlers**: Jobs are registered with an in-memory closure map; the queue itself does not serialise Rust closures.
- **Scheduling**: Each job may have an optional `DailySchedule` (HH:MM local time, stored as UTC).
- **Yielding**: Jobs marked `yield_on_user_activity` can pause between logical boundaries when the user has interacted within the last 5 minutes.
- **Timeout**: `run_now` and scheduled runs are bounded by a configurable timeout (default 120 minutes).

## Public API

- `JobQueue::init(path)` – create or open the queue database.
- `JobQueue::register(job)` – persist a job definition and store its handler.
- `JobQueue::run_now(job_id)` – execute a job immediately, recording the run.
  The corresponding HTTP endpoint (`POST /kb/optimization/run-now`) is restricted to loopback addresses only.
- `JobQueue::status(job_id)` – get schedule and last run for a job.
- `JobQueue::list_jobs()` – list all registered jobs with status.
- `Job::new(id, priority, schedule, yield_on_user_activity, handler)` – construct a job.

## Types

- `JobPriority`: `System`, `Maintenance`, `User`.
- `JobRunStatus`: `Running`, `Succeeded`, `Failed`, `TimedOut`, `Cancelled`.
- `DailySchedule`: parses `"02:00"` and computes the next UTC instant.
- `JobContext`: passed to handlers; currently contains the `job_id`.

## Integration

The daemon initialises the queue in `AppState::from_config_with_llm`, registers the `knowledge.optimization` job, and spawns no dedicated scheduler loop yet (daily scheduling is a follow-up). Manual triggers are available via the CLI and HTTP API.
