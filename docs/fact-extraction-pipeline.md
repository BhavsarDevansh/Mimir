# Fact Extraction Pipeline

> **Issue:** #55  
> **Phase:** 2 — Knowledge Graph  
> **Version:** 0.29.0

## Overview

The fact extraction pipeline transforms a raw user message into structured, validated, and stored facts in the knowledge graph. The pipeline is deterministic Rust; the LLM only provides structured extraction output. All validation, confidence assignment, entity resolution, and insertion decisions are made in Rust.

## Trigger

The pipeline is triggered automatically by both chat endpoints (`/chat` and `/chat/stream`) after a successful, non-incognito turn. The chat route submits a `LibrarianGoal` to the `AgentRuntime`, which dispatches the [`Librarian Agent`](../librarian-agent.md) in the background so the HTTP response is never delayed. The Librarian receives the full conversation turn, the configured user identity, the current condensed memory, and recent related facts from the knowledge graph. Incognito sessions skip extraction to avoid polluting the knowledge graph.

Additionally, the LLM can proactively call the `remember` tool during conversation. This gives the LLM explicit write access to the knowledge graph, letting it persist facts immediately rather than waiting for the background pipeline.

## Architecture

```
User message
    → LLM extraction ("remember" tool)
    → Rust validation (schema, entity types, temporal bounds)
    → Entity resolution (exact → alias → FTS5 fuzzy → create)
    → Dedup / corroboration check (stub for #79)
    → Confidence assignment (classification → SourceType → confidence::initial)
    → Correction handling (temporal or retrospective)
    → Sensitive gate (Disputed + pending_confirmation)
    → Fact insertion + source attachment + audit log
    → Inference engine trigger
```

## Files

- `mimir-knowledge/src/extract.rs` — pipeline implementation
- `mimir-knowledge/src/lib.rs` — `KnowledgeGraph` facade methods
- `mimir-knowledge/tests/extraction_test.rs` — 11 integration tests
- `mimir-knowledge/src/db/migrations/026_add_pending_confirmation.sql`
- `mimir-knowledge/src/db/migrations/027_add_rejected_change_type.sql`

## LLM Extraction

The `remember` tool schema is a JSON object with a `facts` array. Each fact contains:

| Field | Description |
|-------|-------------|
| `classification` | `Explicit`, `Casual`, or `Correction` |
| `subject` | Entity name |
| `subject_type` | `Person`, `Place`, `Event`, `Object`, `Concept`, `Organization`, `Activity`, `DateTime` |
| `predicate` | Relationship or property |
| `object` | Value or entity name |
| `object_is_entity` | Boolean |
| `object_type` | Entity type (if `object_is_entity` is true) |
| `temporal` | Optional `valid_from` / `valid_until` (ISO-8601) |
| `is_sensitive` | Boolean |
| `correction_scope` | ISO-8601 datetime or `"always"` |

The extraction prompt defines role, schema, classification criteria, and sensitive categories. It contains **no conditional logic, no workflow instructions, and no "if X then Y"** — all of that lives in Rust.

## Rust Validation

1. **Schema conformance:** Deserialize into `ExtractedFact` via serde. Invalid fields cause the individual fact to be rejected.
2. **Entity type validation:** `subject_type` and `object_type` must parse to a known `EntityType` variant.
3. **Temporal parsing:** `valid_from` / `valid_until` must be valid ISO-8601 if present.

## Entity Resolution

1. Search by name via `queries::entity::get_by_name` (exact → alias → FTS5 fuzzy).
2. If found, use the existing entity ID.
3. If not found, create a new entity with the LLM-provided type.

## Confidence Assignment

Confidence is **never** taken from the LLM. It is derived from classification:

| Classification | SourceType | Confidence |
|----------------|-----------|------------|
| Explicit | `UserEdit` | `1.0` |
| Casual | `Interaction` | `0.30` |
| Correction | `UserEdit` | `1.0` |

## Correction Handling

### Temporal Correction
- `correction_scope` is parsed as an ISO-8601 datetime.
- The new fact's `valid_from` is set to that datetime.
- The existing `insert_fact_in_tx` temporal-overlap logic automatically closes the sole open-ended predecessor at that datetime.

### Retrospective Correction (`"always"`)
- All overlapping `Active` facts with the same `subject_id + predicate_id` are found.
- Each is marked as `Corrected` via `set_status`.
- Each is then moved to trash via `forget_fact` (soft-delete with cascade to inferred children).
- The new fact is inserted as `Active` with confidence `1.0`.

## Sensitive Fact Confirmation

Facts flagged as `is_sensitive = true` by the LLM are gated:

1. Inserted as `Disputed` with `pending_confirmation = TRUE`.
2. Added to an in-memory `HashSet<i32>` cache (rebuilt from DB on startup).
3. Returned in `ExtractionOutcome::pending_confirmation`.

### Confirm
`KnowledgeGraph::confirm_fact(fact_id)`:
- Verifies `pending_confirmation = TRUE`.
- Updates status to `Active`, confidence to `1.0`, `pending_confirmation = FALSE`.
- Writes a `StatusChange` audit entry.
- Removes from cache.
- Triggers the inference engine.

### Reject
`KnowledgeGraph::reject_fact(fact_id)`:
- Verifies `pending_confirmation = TRUE`.
- Writes a `Rejected` audit entry.
- Hard-deletes the fact (sources cascade; audit rows persist).
- Removes from cache.

### Auto-Cleanup
Nightly optimization (#58) will query `pending_confirmation = TRUE AND updated_at < now() - 7 days` and auto-reject stale items.

## Inference Trigger

After each non-sensitive fact insertion, the rule engine evaluates all registered rules against the new fact. Any inferred `NewFact`s are inserted via `insert_fact_internal`, which handles cycle detection, confidence calculation, and recursive cascade.

## Public API

```rust
pub async fn extract_facts(
    &self,
    llm: &Arc<dyn LlmBackend>,
    user_message: &str,
) -> Result<ExtractionOutcome, KnowledgeError>

pub async fn confirm_fact(&self, fact_id: i32) -> Result<Fact, KnowledgeError>
pub async fn reject_fact(&self, fact_id: i32) -> Result<(), KnowledgeError>
```

## Testing

11 integration tests in `mimir-knowledge/tests/extraction_test.rs`:

1. `test_explicit_extraction` — Active, confidence 1.0, source attached.
2. `test_casual_extraction` — Confidence 0.30, Disputed on overlap.
3. `test_entity_resolution_existing` — Reuses existing entity.
4. `test_entity_creation_new` — Creates new entity with correct type.
5. `test_temporal_correction` — Old fact gets `valid_until` bounded.
6. `test_retrospective_correction` — Old fact marked `Corrected`, moved to trash.
7. `test_sensitive_fact_confirmation` — Disputed + pending, confirm → Active.
8. `test_multiple_facts` — Multiple facts processed in one message.
9. `test_invalid_llm_output` — Malformed JSON returns `Err`.
10. `test_empty_extraction` — Empty facts array returns empty outcome.
11. `test_reject_sensitive_fact` — Rejection deletes fact and clears cache.

All tests use `MockLlmClient` with `mimir-core`'s `mock-llm` feature for deterministic, fast validation.

## Predicate Resolution (v0.50.0)

During extraction, each fact's `relationship_type` is resolved through `KnowledgeGraph::ensure_relationship_type`, which trims/lowercases the name (via `normalize_alias`), looks it up in the `relationship_type_aliases` table — the single source of truth — and, on a miss, auto-creates a canonical `relationship_types` row plus a self-alias. The resolved canonical name then drives `split_list_objects`. LLM synonyms such as `attended`, `hobbies`, or `works_for` therefore map to their canonical types (`studied_at`, `hobby`, `works_at`) purely from seeded data — there is no hardcoded synonym map in code.

The batch processor (`process_fact_batch`, shared by `extract_facts`/`extract_facts_with_context` and the `remember` tool entrypoint `process_remember_output`) tolerates predicate-resolution errors per-fact: one invalid predicate is recorded in `ExtractionOutcome::errors` without aborting the rest of the batch.

> **Issue #136:** the deprecated hardcoded `normalize_predicate` map and the duplicate `normalize_relationship_type` snake_case helper were removed from `mimir-knowledge/src/extract.rs`. Migrations `036_seed_relationship_type_aliases.sql` and `037_seed_core_predicates_and_aliases.sql` seed every legacy synonym as data, so behaviour is unchanged for `attended`→`studied_at`, `hobbies`→`hobby`, etc. A side effect of routing through `ensure_relationship_type` is that an unknown predicate on a fact that is later rejected (e.g. invalid `subject_type`) still registers its canonical type; this is intentional and idempotent.

The `LIST_PREDICATES` allow-list was expanded to include `has_pets`, `has_child`, `has_parent`, `has_sibling`, and `has_partner` so comma-separated values for these predicates are correctly split into individual facts.
