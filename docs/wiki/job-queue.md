# Background Jobs

## What is it?

Mimir runs maintenance tasks in the background so your knowledge graph stays clean and accurate without manual intervention. A unified scheduler ensures these tasks never interfere with your conversations.

## How it works

Background work runs through two subsystems with different rules: the scheduler for scheduled jobs, and the hooks engine for event-driven hooks.

Queue database connections use SQLite WAL mode with `synchronous=NORMAL` and a 10,000-page cache, so frequent job-status writes avoid SQLite's full-fsync default while WAL preserves database consistency; a power loss can roll back recent committed queue-status writes.

**Scheduled jobs** (nightly optimization, pending-fact cleanup, events scan) all follow the same lifecycle:

1. **Deduplication** — if a job is already waiting or running, it is not queued again.
2. **Debounce** — after a job is requested, the scheduler waits a short time (default 5 s) to batch rapid successive requests.
3. **Cooldown** — the scheduler then waits until you have not interacted with Mimir for a cooldown period (default 60 s).
4. **Idle gate** — finally, it checks that the LLM worker pool is completely idle before dispatching.

**Event-driven hooks** apply per-hook queue policies instead of one shared pipeline: `remember.chat` debounces per session (default 10 s) and is idle-gated with the scheduler cooldown; `memory.condensation` reuses the scheduler's debounce and cooldown with the idle gate; `connector_item.remember` enqueues every staged item in FIFO order and is ungated, with LLM calls routed through the shared worker pool's system queue.

## Current jobs

- **Memory condensation** — the `memory.condensation` hook, triggered automatically when facts change, or manually via `mimir memory --refresh`.
- **Knowledge graph optimization** — runs every night (default 02:00) to deduplicate facts, resolve contradictions, recalculate confidence, and clean up old data.

## How to check status

```bash
mimir kb optimization --status
```

Shows the job schedule, next run time, and the result of the last run.

## How to run manually

```bash
mimir kb optimization --run-now
```

Triggers the full optimization pipeline immediately. This can take a few minutes depending on graph size.

If the run is cancelled (for example by daemon shutdown) the request returns `409 Conflict`; if it exceeds the configured timeout it returns `504 Gateway Timeout`.

```bash
mimir memory --refresh
```

Triggers memory condensation immediately, bypassing the hook's debounce and cooldown.

## Shutdown behaviour

When Mimir shuts down, any background job that is still running is asked to stop. Jobs that cooperate with the cancellation signal finish their current step cleanly and exit. Cancellation is best-effort: the signal only asks the job to stop, and the job's thread is neither aborted nor joined, so synchronous or blocking work can keep running until it finishes. The run is recorded as `cancelled` in the job history, so `mimir kb optimization --status` shows what happened.

## Resource limits

The nightly optimization job runs with the resource limits from `[knowledge.optimization]` in `config.toml`: `cpu_cores` (how many CPUs it may use, Linux), `nice_level` (a signed Unix priority value: positive values lower scheduling priority, negative values raise it and may require additional privileges), and the optional `memory_limit_mb` (a best-effort memory cap on Linux systems with a writable cgroup v2 setup). The memory cap applies to the whole Mimir process while the job runs, not just the job's thread. These limits are best-effort — if your system cannot apply one, Mimir logs it and runs the job anyway.

## Best practices

- Let the nightly schedule handle routine maintenance.
- Run optimization manually only when you want to force cleanup after a large import or batch operation.
- The daemon must be running for `--status`, `--run-now`, and `--refresh` to work.
- If you are actively chatting, background jobs will automatically wait until you are done.
