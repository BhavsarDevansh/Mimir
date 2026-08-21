# Fact Extraction Pipeline

> **Issues:** #55, #181 (Phase 3 F4 — shared normalize/insert boundary)
>
> **Phase:** 2 — Knowledge Graph (boundary shared with Phase 3 connectors)
>
> **Version:** 0.65.0

## Overview

The fact extraction pipeline transforms a raw user message into structured, validated, and stored facts in the knowledge graph. The pipeline is deterministic Rust; the LLM only provides structured extraction output. All validation, confidence assignment, entity resolution, and insertion decisions are made in Rust.

## Trigger

Learning is **LLM-orchestrated** (Issue #137). The conversational LLM calls the `remember` tool during the chat turn to persist facts it judges worth keeping, so extraction happens inline as part of the response and does not learn from chitchat. The deterministic Rust pipeline (validation, confidence, entity resolution, sensitive gating, insertion) runs when the tool executes — the LLM only supplies structured facts; it cannot set confidence or override policy.

The [`Librarian Agent`](../librarian-agent.md) and `KnowledgeGraph::extract_facts_with_context` remain as an on-demand library API (for future bulk import or specialist agents) but are no longer auto-invoked after every turn. Incognito sessions never learn.

## Architecture

```
User message
    → LLM extraction ("remember" tool)                 [extract/pipeline.rs]
    → Conversational adapter:
        predicate canonicalisation + list splitting
        + parse LLM string fields → NormalizedFact       [extract/parse.rs]
    → normalize_and_insert(kg, Vec<NormalizedFact>, Provenance)   [normalize.rs]
        → entity resolution (exact name → alias → FTS5 fuzzy ≥ 0.9 → create new,
           restricted to the requested entity type; Phase 3 F5 / #182)
        → confidence = confidence::initial(source_type, connector_type)
        → correction handling (temporal or retrospective)
        → sensitive gate (Disputed + pending_confirmation)
        → fact insertion + source attachment + audit log
            (corroboration detected here — see Corroboration below)
        → inference engine trigger
```

Connectors build `NormalizedFact`s directly from structured/LLM-extracted items and call the same `normalize_and_insert` with a connector `Provenance`, so connector-sourced facts get identical confidence scoring, corroboration, supersession, and sensitivity gating as facts you tell Mimir directly.

## Corroboration (#79)

Corroboration is resolved **inside `insert_fact_in_tx`** for every insert path (extraction, batch insert, direct `KnowledgeGraph::insert_fact`), within the same transaction as supersession. When a new **non-explicit** fact covers the same claim as an existing fact — same `subject_id + relationship_type_id + object`, temporally overlapping `valid_from`/`valid_until` — and the existing fact is `Active` (or awaiting confirmation):

1. **No new facts row** is created; the existing fact is returned.
2. A new `sources` row is inserted against the existing fact (provenance).
3. If the existing fact is **non-explicit and non-inferred**, its confidence is boosted by `+0.05`, capped at `0.95`. Explicit facts stay at `1.0`; inferred fact confidence is structural (recalculated from parents) and is not boosted.
4. `SourceAdded` and `ConfidenceChange` audit entries are written and `stale_confidence` is cleared; the confidence change cascades to all inferred children comprehensively within the transaction.

The corroboration path runs **before** supersession, so an explicit statement still supersedes rather than corroborates. A re-statement from an identical source (`(source_type, connector_instance_id, raw_reference)` already recorded) is a **no-op** — it is not an independent source and would collide with the `sources` UNIQUE index. Non-overlapping temporal ranges never corroborate; they form a timeline of separate facts.

## Shared normalize/insert boundary (#181, Phase 3 F4)

The resolve → confidence → sensitivity-gate → insert orchestration is extracted from the conversational path into a single reusable function so that chat learning and connector ingestion funnel through one deterministic Rust pipeline:

```rust
pub async fn normalize_and_insert(
    kg: &KnowledgeGraph,
    facts: Vec<NormalizedFact>,
    provenance: Provenance,
) -> Result<ExtractionOutcome, KnowledgeError>
```

- **`Provenance`** (one per call) carries the batch-level origin: the connector instance id + connector type (for connector syncs) and the `extraction_method` (`LlmExtraction` for chat, `StructuredParse` for structurally-parsed connector items). Conversational learning uses `Provenance::chat`.
- **`NormalizedFact`** (one per fact) carries the typed fact content — entity types, parsed temporal bounds, typed `RecurrenceType`, validated category ids, the sensitivity flag, the optional correction scope, and the per-fact `raw_reference` (the native source item id, e.g. an email UID). `source_type` is per-fact because a single chat batch may mix `Explicit` (`UserEdit`) and `Casual` (`Interaction`) facts; connectors set `Connector`.
- **Confidence** is `confidence::initial(source_type, connector_type)` — the per-source-type / per-connector reliability score with **no extraction-method discount**. Corroboration, supersession, and inference are inherited for free from `insert_fact_in_tx`, so a cross-connector corroboration (e.g. a Gmail flight fact + a Calendar event on overlapping dates) adds a source and boosts confidence without creating a duplicate fact.

The conversational adapter (`extracted_to_normalized` in `extract/pipeline.rs`) does the LLM-output normalisation the shared boundary cannot: predicate canonicalisation (so list-splitting sees canonical names), list splitting, and parsing the LLM's string-typed fields into the typed `NormalizedFact`. Per-fact canonicalisation and parse errors are tolerated and surfaced via `ExtractionOutcome::errors`, preserving the previous batch behaviour.

The tool-call + fence-fallback parsing itself is shared with the connector extraction path (issue #259): `mimir-core::llm::parse_tool_output` owns the three-step dance (first `tool_calls` entry's `function.arguments`, else fence-stripped `content`, else error) once, and both `parse_remember_output` and the Email connector's `parse_output` map its `ToolOutputParseError` onto their own error types. The conversational path keeps its bare-`Vec<ExtractedFact>` fallback on top of the shared parser. The `remember` tool schema is built once via `LazyLock` and shared by every extraction call.

## Files

- `mimir-knowledge/src/extract/` — conversational half: `remember` tool schema, extraction prompts, LLM-output parsing, and the adapter that maps `ExtractedFact` onto `NormalizedFact`/`Provenance`
- `mimir-core/src/llm/tool_output.rs` — shared LLM tool-output parsing (`parse_tool_output` + `ToolOutputParseError`), used by both the conversational and connector extraction paths
- `mimir-knowledge/src/normalize/` — shared `normalize_and_insert` boundary (entity resolution, confidence, sensitivity gate, insertion, event overlay) used by both chat and connectors
- `mimir-knowledge/src/queries/fact/` — `insert_fact_in_tx`, corroboration + supersession paths
- `mimir-knowledge/src/confidence.rs` — structural confidence model + transactional confidence cascade
- `mimir-knowledge/src/sensitivity.rs` — deterministic sensitivity gate (category + content checks)
- `mimir-knowledge/src/lib.rs` — `KnowledgeGraph` facade methods
- `mimir-knowledge/tests/extraction_tool_test.rs`, `extraction_text_fallback_test.rs`, `extraction_rust_overrides_test.rs` — conversational extraction integration tests
- `mimir-knowledge/tests/normalize_test.rs` — shared-boundary integration tests (connector insert + cross-connector corroboration)
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
| `correction_scope` | ISO-8601 datetime, `"always"`, or omitted (defaults to a temporal correction at `now` for `Correction` facts) |

The extraction prompt defines role, schema, classification criteria, and a softened sensitivity instruction ("Flag health, financial, relationship, religious, political, or legal facts. Mimir will validate your assessment."). It contains **no conditional logic, no workflow instructions, and no "if X then Y"** — all of that lives in Rust.

## Rust Validation

1. **Schema conformance:** Deserialize into `ExtractedFact` via serde. Invalid fields cause the individual fact to be rejected.
2. **Entity type validation:** `subject_type` and `object_type` must parse to a known `EntityType` variant.
3. **Temporal parsing:** `valid_from` / `valid_until` must be valid ISO-8601 if present.

## Entity Resolution

1. Search by name via `queries::entity::get_by_name_typed` (exact name → exact alias → FTS5 fuzzy), restricted to entities of the LLM-provided `subject_type` / `object_type`. Cross-type matches are dropped so a name like "Apple" resolved as a `Concept` never merges into the `Organization` "Apple Inc".
2. Apply the resolution policy (`pick_resolution`): an exact-name or exact-alias hit always resolves to the existing entity; a fuzzy hit resolves only when its normalised score is ≥ `FUZZY_RESOLVE_THRESHOLD` (0.9), otherwise it is treated as a miss. Results are sorted by score descending (alias 1.1 > exact name 1.0 ≥ fuzzy), so only the best same-type candidate is inspected.
3. If no candidate resolves, create a new entity with the LLM-provided type.

> **Phase 3 F5 / #182:** the chain is shared by chat extraction and connector ingestion. Entity names are globally unique (case-insensitive) by schema, so the cross-type guard matters for token-overlap / fuzzy matches rather than identical names. Alias creation stays explicit via `preferred_name`; fuzzy resolution does not auto-learn aliases.

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

### Scope-less Correction (`None`)

- The shared `normalize_and_insert` boundary gates corrections on the `is_correction` flag (set by the chat adapter from the LLM `Correction` classification), **not** on `correction_scope` being present.
- When the LLM emits `Correction` but omits `correction_scope`, `handle_correction` receives `None` and defaults the new fact's `valid_from` to `now`, so the correction takes effect from the current moment onward.
- The insert temporal-overlap logic then closes the sole open-ended predecessor at `now`, mirroring the explicit-datetime path.
- Connectors never set `is_correction`, so this path is conversational-only.

## Sensitive Fact Confirmation

### Rust Sensitivity Gate (#142)

Sensitivity is validated in Rust, not delegated to the LLM. The LLM provides an initial `is_sensitive` flag, but Rust applies a deterministic **AND gate** using two independent signals in `mimir-knowledge/src/sensitivity.rs`:

1. **Category check** (`is_sensitive_by_category`) — does the fact belong to a known sensitive catalogue category? The `SENSITIVE_CATEGORIES` constant lists the Dewey-Decimal category IDs that require confirmation (health, allergies, financial, romantic, cultural/religious, values/philosophy).
2. **Content check** (`is_sensitive_by_content`) — does the fact's object text contain a sensitive keyword as a **whole word** (e.g. "allergic", "diabetes", "salary", "debt", "divorce", "citizenship")? Word-boundary matching prevents benign words that merely contain a keyword (e.g. "hospitality" contains "hospital", "indebted" contains "debt", "visage" contains "visa") from being confirmed sensitive. This is the fallback for miscategorised facts.

The combined `is_sensitive(llm_flag, category_ids, object)` function implements:

| LLM says | Rust says | Result |
|----------|-----------|--------|
| sensitive | sensitive | **sensitive** |
| sensitive | non-sensitive | **non-sensitive** (Rust overrides) |
| non-sensitive | anything | **non-sensitive** |

Rust can only **narrow** the LLM's assessment — it never flags a fact as sensitive when the LLM did not. This eliminates the false-positive problem where benign preferences ("I don't like chihuahuas", "I live in a small flat") were routed into pending confirmation.

### Pending Flow

Facts that pass the sensitivity gate are:

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

Conversational entrypoints (in `mimir_knowledge::extract`):

```rust
pub async fn extract_facts(
    &self,
    llm: &Arc<dyn LlmBackend>,
    user_message: &str,
) -> Result<ExtractionOutcome, KnowledgeError>

pub async fn process_remember_output(
    kg: &KnowledgeGraph,
    output: RememberOutput,
) -> Result<ExtractionOutcome, KnowledgeError>

pub async fn confirm_fact(&self, fact_id: i32) -> Result<Fact, KnowledgeError>
pub async fn reject_fact(&self, fact_id: i32) -> Result<(), KnowledgeError>
```

Shared boundary (in `mimir_knowledge::normalize`), used by both chat and connectors:

```rust
pub async fn normalize_and_insert(
    kg: &KnowledgeGraph,
    facts: Vec<NormalizedFact>,
    provenance: Provenance,
) -> Result<ExtractionOutcome, KnowledgeError>
```

`ExtractionOutcome` and `PendingFact` are defined in `mimir_knowledge::normalize` and re-exported from `mimir_knowledge::extract` for existing callers.

## Testing

11 integration tests across `mimir-knowledge/tests/extraction_tool_test.rs`, `extraction_text_fallback_test.rs`, and `extraction_rust_overrides_test.rs`:

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

## Predicate Resolution (v0.126.0)

During extraction, each fact's `relationship_type` is resolved through `KnowledgeGraph::resolve_canonical_relationship_type`, which enforces the Rust-side canonical predicate allow-list (`CANONICAL_PREDICATES`, issue #401): the name is trimmed/lowercased (via `normalize_alias`), looked up in the `relationship_type_aliases` table — the single source of truth — and the resolved canonical type must be part of the seeded set. LLM synonyms such as `attended`, `hobbies`, or `works_for` therefore map to their canonical types (`studied_at`, `hobby`, `works_at`) purely from seeded data — there is no hardcoded synonym map in code. The prompt-instructed `favourite_<thing>` family is accepted as an open set (auto-creating the specific favourite on first use); any other unknown predicate is rejected with a clear error instead of auto-creating a `relationship_types` row, so an LLM-invented predicate can never pollute the ontology. The resolved canonical name then drives `split_list_objects`.

The batch flow (`extracted_to_normalized` → `normalize_and_insert`, shared by `extract_facts`/`extract_facts_with_context` and the `remember` tool entrypoint `process_remember_output`) tolerates predicate-resolution errors per-fact: one invalid predicate is recorded in `ExtractionOutcome::errors` without aborting the rest of the batch.

After resolution, the shared write boundary enforces the seeded subject/object entity-type constraints (`relationship_constraints`, issue #402): an entity-object fact whose (subject, object) pair is not in the seeded allow-list for its predicate is rejected with `KnowledgeError::InvalidRelationshipConstraint`, reported as a per-fact error in `ExtractionOutcome::errors` exactly like the predicate allow-list rejection. Predicates without seeded constraints and literal-object facts are unaffected, so connector-emitted predicates and value-style facts (e.g. `works_as` → "Software Engineer") keep flowing.

> **Issue #136:** the deprecated hardcoded `normalize_predicate` map and the duplicate `normalize_relationship_type` snake_case helper were removed from `mimir-knowledge/src/extract/`. Migrations `036_seed_relationship_type_aliases.sql` and `037_seed_core_predicates_and_aliases.sql` seed every legacy synonym as data, so behaviour is unchanged for `attended`→`studied_at`, `hobbies`→`hobby`, etc. The old side effect of `ensure_relationship_type` — an unknown predicate registering a canonical type even when its fact was later rejected — now applies only to the permissive connector boundary; the conversational path rejects unknown predicates outright (issue #401 below).
> **Issue #401:** the conversational extraction path no longer auto-creates unknown predicates. `resolve_canonical_relationship_type` accepts only seeded canonical predicates, their aliases, and the prompt-instructed `favourite_<thing>` family; anything else (e.g. an LLM-invented `moved_into`) is rejected with a `Validation` error collected in `ExtractionOutcome::errors`, and no `relationship_types` row is created. Migration `050_seed_canonical_predicates_and_reconcile.sql` seeds the predicates the extraction path legitimately uses that were missing (`skill`, `has_appointment`, and the sensitive predicates migration `029` intended to mark), and deletes auto-created types no fact references. The shared `normalize_and_insert` boundary stays permissive for connector-provenance facts, but the connector surface is now seed-pinned (issue #412): migration `053` seeds every deterministic connector-emitted predicate as canonical with constraints, `mimir-connectors` tests assert extractor output uses only canonical vocabulary, and the email LLM layer rejects non-canonical predicates, so no connector path can silently auto-create a `relationship_types` row.

## List Splitting (v0.128.0)

`split_list_objects` in `mimir-knowledge/src/extract/parse.rs` expands a fact whose object literal is a comma-separated list into one fact per item, so the LLM cramming `hobby → "Geopolitics, Software Development, Tech"` into a single fact still yields three independent `hobby` facts. The predicate allow-list is the single shared `MULTI_VALUED_PREDICATES` constant in `mimir-knowledge/src/graph/predicates.rs` (issue #405): the same constant drives the insert overlap logic in `queries/fact/insert.rs` (distinct objects for these predicates coexist instead of superseding), so the splitter and the multi-valued insert semantics can never drift apart. The open `favourite_<thing>` family is split generically by prefix — `favourite_movie → "Inception, Interstellar"` becomes two facts — matching the prompt's `favourite_{{thing}}` standard instead of only the two named favourites; the same shared `is_favourite_family_predicate` helper also marks the family multi-valued at insert, so both split facts stay active. The extraction prompt keeps a single high-level rule ("Emit one fact per list item") and no longer carries the BAD/GOOD parsing example, so list-splitting is enforced deterministically in Rust rather than delegated to prompt engineering.
