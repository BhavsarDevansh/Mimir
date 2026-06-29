# Inference Engine

## Architecture

The inference engine is a lightweight, Rust-native rule system embedded in `mimir-knowledge`. It runs automatically after every fact insertion and during nightly batch re-evaluation.

### Components

- `InferenceRule` trait — async evaluation against a single fact
- `RuleEngine` — holds a `Vec<Box<dyn InferenceRule>>` and orchestrates evaluation
- `CascadeContext` — `HashSet` tracking processed triples to prevent infinite loops
- V1 rules: `Transitivity`, `Contradiction`, `Threshold`

### Cascade Behavior

When `KnowledgeGraph::insert_fact` commits a fact, it automatically calls `RuleEngine::evaluate_insert`. Each inferred `NewFact` is recursively inserted with the same `CascadeContext`. Duplicate triples are skipped. Cascade depth is unbounded; cycle detection is the only safety net.

### Confidence

Inferred facts use `confidence::inference_confidence(parents, depth, num_parents)`:

- `parents`: `(confidence, is_positive)` tuples from source facts
- `depth`: `max(parent_a.depth, parent_b.depth) + 1`
- Chain penalty: `0.8^depth`
- Breadth factor: `0.6` (1 parent), `0.75` (2), `0.9` (3), `1.0` (4+)
- Result is clamped to `[0.0, 0.95]`

## Rule Descriptions

### Transitivity

**Trigger:** `visited` or `is_in`

**Forward:** `A-P-B` inserted + `B is_in C` exists → infer `A-P-C`

**Backward:** `B is_in C` inserted + `A visited B` exists → infer `A visited C`

**Backward `is_in` lookup is intentionally disabled** to prevent self-disputing chains.

### Contradiction

**Real-time:** During `insert_fact`, overlapping non-explicit facts mark both sides `Disputed` and write `Contradicts` edges in both directions.

**Batch:** `evaluate_batch` scans `Disputed` pairs linked by `Contradicts`. If one is explicit (non-inferred, e.g., source == UserEdit or source == System) and the other is inferred, the inferred is marked `Superseded` and the explicit `Active`.

### Threshold

**Trigger:** `rejected_action`

**Logic:** Count active `rejected_action` facts for same `(subject, object)`. If count ≥ 3, upsert a preference with:

- `category = General`
- `key = reject_<object>`
- `value = true`
- `confidence = 0.70`

**Nightly re-count:** If count drops below 3, write an audit log entry with reason `"threshold no longer met; review recommended"`.

## Fact Dependencies

Inferred facts write `InferredFrom` edges in `fact_dependencies` for each parent. `parent_fact_ids` on `NewFact` drives this.

## Nightly Optimization

`run_nightly_optimization` orchestrates passes in order:

1. Contradiction auto-resolution (explicit > inferred)
2. Root-aware confidence recalculation for `stale_confidence = true` facts (`confidence::recalculate_stale_fact`): each stale fact is recalculated/cleared itself and the change is cascaded to inferred descendants in one transaction. See [Nightly Knowledge Graph Optimization](nightly-optimization.md) for the full pass list.
3. Inference re-evaluation (`RuleEngine::evaluate_batch`)
4. Threshold nightly re-count

Dedup, dormant cleanup, and compaction are left as `TODO` stubs.
