# Nightly Knowledge Graph Optimization

## Overview

The nightly optimization pipeline maintains graph health by running a fixed sequence of 10 passes over the knowledge graph (seven core optimization passes plus three cleanup steps). It is implemented in `mimir-knowledge/src/optimization/mod.rs` and orchestrated by `OptimizationRunner`.

## Passes

1. **Deterministic Deduplication** – merges facts with identical subject-predicate-object triples, boosting confidence and preserving sources.
2. **Semantic Deduplication** – sends near-match candidates to the LLM with a strict JSON schema. Auto-merges pairs with confidence >= 0.9; queues uncertain pairs in `dedup_queue`.
3. **Contradiction Resolution** – evaluates explicit vs inferred facts using `ContradictionRule`.
4. **Inference Chain Re-evaluation** – runs the rule engine (`TransitivityRule`, `ContradictionRule`) and inserts newly inferred facts. Includes `ThresholdRule` nightly re-count.
5. **Confidence Recalculation** – for each stale fact (`stale_confidence = TRUE`), runs a root-aware recalculation (`confidence::recalculate_stale_fact`): it recalculates the stale row itself from its parents (inferred) or just clears the flag (non-inferred), writes a `ConfidenceChange` audit entry only when confidence actually changes, and cascades the result to inferred descendants within the same transaction. This prevents the pass from leaving the selected stale rows unrecalculated while only updating their children.
6. **Dormant Cleanup** – forgets old disputed non-user facts that have a higher-confidence counterpart.
7. **Pattern Consolidation** – currently a stub; will group repeated fact patterns in a future release.
8. **Pending Confirmation Cleanup** – hard-deletes pending-confirmation facts older than 7 days.
9. **Trash Cleanup** – permanently removes expired trash rows.
10. **Compaction** – rebuilds FTS5 index, runs `ANALYZE`, and `VACUUM`s the database.

## Transaction Model

Each mutating operation runs in its **own short transaction** rather than one transaction wrapping an entire pass. For example, deduplication and semantic-dedup both `begin()` / `commit()` per fact pair (`merge_fact_pair`), so a single merge failure cannot roll back an entire pass worth of work. Confidence recalculation, by contrast, recalculates a stale root **and** cascades to its inferred descendants within a single transaction, so a root and its subtree are always consistent. Each pass records its outcome (facts merged, candidates queued, facts forgotten, or error) in `optimization_pass_runs` regardless of per-item success.

## Backup

Before the first pass, `VACUUM INTO` creates a dated backup at `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db`. If the file already exists, a numeric suffix is appended. The filename is reserved atomically (`O_EXCL`), so concurrent runs sharing a backup directory can never collide (issue #241); a reserved file is removed again if the `VACUUM INTO` fails, and `prune_backups` skips entries that vanish mid-scan. Rotation (keep last 7 daily + 4 Sunday weekly) is planned as follow-up work.

## Configuration

```toml
[knowledge.optimization]
cpu_cores = 1
nice_level = 10
timeout_minutes = 120
schedule_time = "02:00"
# memory_limit_mb = 2048  # Optional: best-effort cgroup v2 memory cap (MiB)
```

`cpu_cores` and `nice_level` are enforced per run as best-effort `JobResourceLimits` (Linux CPU affinity and POSIX scheduling priority; see `docs/job-queue.md`). `memory_limit_mb` is an optional best-effort cgroup v2 memory cap applied while the optimization job runs — it requires a writable (delegated) cgroup v2 filesystem and is skipped with a debug log otherwise. The cap is process-wide: the whole daemon is moved into the job cgroup, so it limits the entire process while the job runs. None of the limits can fail the job.

## Background Scheduler Integration

The optimization job is registered in the durable `JobQueue` with a daily schedule at `JobPriority::System` (the lowest-priority class, so it never preempts user or connector work). The `JobPriority` classes are:

| Priority | Value | Meaning |
|---|---|---|
| `System` | 0 | Daemon maintenance — never competes with active user work (optimization, condensation) |
| `Maintenance` | 1 | Connector sync and background upkeep |
| `User` | 2 | Explicitly requested user jobs |

The `BackgroundScheduler` polls for scheduled jobs every 60 seconds. When the optimization job is due, it is submitted through the scheduler and follows the same dedupe/debounce/idle rules as any other background job.

After optimization completes, its callback submits `DaemonJob::MemoryCondensation` through the scheduler (not directly via `JobQueue::run_now`), ensuring condensation also waits for user downtime before running.

## Trigger & Daemon-Down Handling

Optimization is a **daemon-only** job: it is submitted and executed by the `BackgroundScheduler` running inside the daemon process, so it never runs when the daemon is down. There is no CLI-triggered optimization path that needs daemon-down recovery; the scheduler simply waits for the next due time. `mimir kb optimization --run-now` submits the job through the same scheduler (HTTP route → `DaemonJob::KnowledgeOptimization`), which queues it subject to the same idle/cooldown gates. If the daemon is not running, the CLI surfaces the standard daemon-down prompt (see [Daemon Auto-Start](wiki/daemon-auto-start.md)).

## Yielding on User Activity

`OptimizationRunner::run_all_with_yield` accepts a `should_yield` closure. The daemon supplies a closure that returns `true` when the last chat interaction is within 5 minutes, causing the runner to sleep for 5 seconds between passes. This is now complemented by the scheduler's cooldown gate, which prevents the job from starting at all if the user is active.

## Run Recording

Each pass outcome is inserted into `optimization_pass_runs` with:
- `pass_name`, `status`, `started_at`, `finished_at`
- `facts_merged`, `dedup_candidates_queued`, `facts_forgotten`
- `error` (on failure)

## Compatibility

`run_nightly_optimization(kg, backup_dir)` is a thin wrapper around `OptimizationRunner::new(...).run_all()` for existing callers. Callers must supply the backup directory explicitly — tests pass a per-test tempdir so the shared real data directory is only ever written by the daemon's scheduled job (issue #241).
