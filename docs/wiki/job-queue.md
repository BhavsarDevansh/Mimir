# Background Jobs

## What is it?

Mimir runs maintenance tasks in the background so your knowledge graph stays clean and accurate without manual intervention. A unified scheduler ensures these tasks never interfere with your conversations.

## How it works

All background jobs go through the same pipeline:

1. **Deduplication** — if a job is already waiting or running, it is not queued again.
2. **Debounce** — after a job is requested, the scheduler waits a short time (default 5 s) to batch rapid successive requests.
3. **Cooldown** — the scheduler then waits until you have not interacted with Mimir for a cooldown period (default 60 s).
4. **Idle gate** — finally, it checks that the LLM worker pool is completely idle before dispatching.

## Current jobs

- **Memory condensation** — triggered automatically when facts change, or manually via `mimir memory --refresh`.
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

```bash
mimir memory --refresh
```

Triggers memory condensation immediately, bypassing the scheduler's debounce and cooldown.

## Best practices

- Let the nightly schedule handle routine maintenance.
- Run optimization manually only when you want to force cleanup after a large import or batch operation.
- The daemon must be running for `--status`, `--run-now`, and `--refresh` to work.
- If you are actively chatting, background jobs will automatically wait until you are done.
