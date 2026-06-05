# Nightly Knowledge Graph Optimization

## Overview

The nightly optimization pipeline maintains graph health by running a fixed sequence of 10 passes over the knowledge graph (seven core optimisation passes plus three cleanup steps). It is implemented in `mimir-knowledge/src/optimization/mod.rs` and orchestrated by `OptimizationRunner`.

## Passes

1. **Deterministic Deduplication** – merges facts with identical subject-predicate-object triples, boosting confidence and preserving sources.
2. **Semantic Deduplication** – sends near-match candidates to the LLM with a strict JSON schema. Auto-merges pairs with confidence >= 0.9; queues uncertain pairs in `dedup_queue`.
3. **Contradiction Resolution** – evaluates explicit vs inferred facts using `ContradictionRule`.
4. **Inference Chain Re-evaluation** – runs the rule engine (`TransitivityRule`, `ContradictionRule`) and inserts newly inferred facts. Includes `ThresholdRule` nightly re-count.
5. **Confidence Recalculation** – recalculates stale inferred facts and cascades changes.
6. **Dormant Cleanup** – forgets old disputed non-user facts that have a higher-confidence counterpart.
7. **Pattern Consolidation** – currently a stub; will group repeated fact patterns in a future release.
8. **Pending Confirmation Cleanup** – hard-deletes pending-confirmation facts older than 7 days.
9. **Trash Cleanup** – permanently removes expired trash rows.
10. **Compaction** – rebuilds FTS5 index, runs `ANALYZE`, and `VACUUM`s the database.

## Backup

Before the first pass, `VACUUM INTO` creates a dated backup at `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db`. If the file already exists, a numeric suffix is appended. Rotation (keep last 7 daily + 4 Sunday weekly) is planned as follow-up work.

## Configuration

```toml
[knowledge.optimization]
cpu_cores = 1
nice_level = 10
timeout_minutes = 120
schedule_time = "02:00"
```

## Yielding on User Activity

`OptimizationRunner::run_all_with_yield` accepts a `should_yield` closure. The daemon supplies a closure that returns `true` when the last chat interaction is within 5 minutes, causing the runner to sleep for 5 seconds between passes.

## Run Recording

Each pass outcome is inserted into `optimization_pass_runs` with:
- `pass_name`, `status`, `started_at`, `finished_at`
- `facts_merged`, `dedup_candidates_queued`, `facts_forgotten`
- `error` (on failure)

## Compatibility

`run_nightly_optimization(kg)` is a thin wrapper around `OptimizationRunner::new(...).run_all()` for existing callers.
