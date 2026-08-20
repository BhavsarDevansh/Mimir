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

**Trigger:** `visited` or `located_in` (`is_in` is a seeded alias of `located_in` since migration 051)

**Forward:** `A visited B` inserted + `B located_in C` exists → infer `A visited C`

**Backward:** `B located_in C` inserted + `A visited B` exists → infer `A visited C`

**`located_in` does not chain to `located_in`** to prevent cyclic chains.

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

## How to Add a New Rule

Each rule is a unit struct implementing the `InferenceRule` trait in its own file under `mimir-knowledge/src/inference/rules/`:

1. **Create the rule file** — e.g. `mimir-knowledge/src/inference/rules/my_rule.rs`, implement `#[async_trait] impl InferenceRule for MyRule` returning `Vec<NewFact>` from `evaluate(&self, fact, kg)`.
2. **Re-export it** from `mimir-knowledge/src/inference/rules/mod.rs` (`pub mod my_rule;`).
3. **Register it** in `KnowledgeGraph::new` (`mimir-knowledge/src/lib.rs`): `engine.register(Box::new(MyRule));`.
4. **Write tests first (TDD)** — add a test in `mimir-knowledge/tests/inference_tests.rs` using the `TestGraph` helper to seed a small DB and assert the inferred `NewFact`.

Rules are pure async functions of `(Fact, &KnowledgeGraph) -> Vec<NewFact>`. They must be deterministic and side-effect-free — the engine handles insertion, cascade, and cycle detection via `CascadeContext`. Never mutate global state or read the system clock directly; inject a `Clock` where time is needed.

## Fact Dependencies

Inferred facts write `InferredFrom` edges in `fact_dependencies` for each parent. `parent_fact_ids` on `NewFact` drives this.

## Nightly Optimization

The inference engine participates in the nightly optimization run, which executes a fixed sequence of 10 passes (dedup, semantic dedup, contradiction resolution, inference re-evaluation, confidence recalculation, dormant cleanup, pattern consolidation, pending-confirmation cleanup, trash cleanup, and compaction). The inference-relevant passes are:

1. **Contradiction auto-resolution** — `ContradictionRule::evaluate_batch` resolves explicit-over-inferred disputes (explicit > inferred).
2. **Confidence recalculation** — root-aware `confidence::recalculate_stale_fact` for every `stale_confidence = true` fact, cascading to inferred descendants in one transaction.
3. **Inference re-evaluation** — `RuleEngine::evaluate_batch` re-runs every rule over all `Active`/`Inferred` facts and inserts any newly derivable facts.
4. **Threshold nightly re-count** — `ThresholdRule` re-checks the ≥3 rejected-action condition and records an audit entry when it no longer holds.

See [Nightly Optimization](nightly-optimization.md) for the full pass list, transaction model, and backup strategy.
