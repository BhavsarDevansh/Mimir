# Structural Confidence Model

## Overview

Confidence in the Mimir knowledge graph is derived **entirely from graph structure**.
There is zero LLM involvement in confidence computation and zero time-based decay.
This is a core architectural principle (issue #51).

## Design Principles

1. **Confidence is Rust-calculated, never LLM-provided.**
2. **Confidence changes only when the graph changes.** No decay. No age-based recalibration.
3. **Initial confidence is assigned by Rust** based on the learning mode.
4. **Per-connector reliability scores** are tracked and adjusted by user corrections.

## Initial Confidence by Source Type

| Source Type | Initial Confidence |
|---|---|
| `UserEdit` | 1.0 |
| `System` | 1.0 |
| `CasualMention` | 0.30 |
| `Import` | 0.80 |
| `Connector` | Per-connector reliability (default: Gmail=0.85, Calendar=0.90, Photos=0.80, LinkedIn=0.75) |
| `Inference` | Computed from parents (see below) |
| `Email` / `Calendar` / `Photo` / `Message` | Mapped to connector defaults (legacy) |

## Inference Confidence Formula

```rust
confidence = signed_sum(parent_confidences) * 0.8^static_depth * breadth_factor(num_parents)
```

Where:
- `signed_sum = Σ (if is_positive { conf } else { -conf })`
- `static_depth` is set at creation time and never changes
- `breadth_factor(n)`:
  - 0 parents → 0.0
  - 1 parent → 0.6
  - 2 parents → 0.75
  - 3 parents → 0.9
  - 4+ parents → 1.0

The result is clamped to `[0.0, 0.95]` for non-explicit facts. Explicit facts use `1.0`.

## Key Properties

- **Static depth**: `inference_depth` is computed once at creation (`max(parent_depths) + 1`) and never updated. This prevents confidence from rising unexpectedly when a parent is removed.
- **Signed parent weights**: The `is_positive` flag on `fact_dependencies` allows negative contributions (e.g., "doesn't love green foods" opposes "likes basil pesto").
- **Breadth bonus**: More independent parents increase confidence, but losing one still hurts because the weighted sum drops (or rises, for removed negatives).

## Explicit Replacement (Supersession)

When a `UserEdit` fact is inserted and an existing fact on the same `subject_id + predicate_id` has a temporally overlapping range, the existing fact is transitioned to `Superseded` status. Its confidence is preserved. A `fact_dependencies` edge `old → new` with `relation_type = Supersedes` is created. The new fact receives `status = Active`.

This applies regardless of the old fact's source type (inferred, connector, casual, or explicit). Only facts that are already `Superseded` are left untouched.

If the temporal ranges do **not** overlap, both facts remain `Active` (timeline behaviour).

## Cascade Behaviour

When a parent fact is forgotten or its confidence changes:

1. Fetch all inferred children.
2. Recalculate each child's confidence using the formula above.
3. If the change exceeds `0.001`, write the new value and recurse with `depth_budget - 1`.
4. Hard limit: `depth_budget = 5` to prevent runaway cascades.

TODO(#51-followup): Replace eager cascade with an async background worker.

## Connector Reliability

Stored in `connector_reliability` table (one row per `ConnectorType`).

- **Feedback loop:**
  - User corrects a connector fact → `-0.02`
  - Corroborated by independent source → `+0.01`
- Scores are clamped to `[0.0, 1.0]`.
- Only affects **future** extractions; existing fact confidence is never retroactively changed.

## Schema Changes

### Migration 019: `facts` and `fact_dependencies` columns
- `facts.inference_depth INTEGER NOT NULL DEFAULT 0`
- `facts.stale_confidence BOOLEAN NOT NULL DEFAULT FALSE`
- `fact_dependencies.is_positive BOOLEAN NOT NULL DEFAULT TRUE`

### Migration 020: Connector reliability
- `connector_types` lookup table
- `connector_reliability` table
- `sources.connector_type_id` foreign key

### Migration 021: New source types
- `CasualMention`, `Import`, `System` added to `source_types`
