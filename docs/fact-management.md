# Fact Management

> **Scope:** `mimir-knowledge` crate — full fact CRUD, temporal overlap logic, confidence placeholder, cascade forget, and audit logging.
>
> **Issue:** #50

---

## Schema

### `facts` table

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | INTEGER | PK, AUTOINCREMENT |
| `subject_id` | INTEGER | NOT NULL, FK → `entities(id)` |
| `predicate_id` | INTEGER | NOT NULL, FK → `predicates(id)` |
| `object_id` | INTEGER | FK → `entities(id)`, nullable |
| `object_literal` | TEXT | nullable |
| `valid_from` | TIMESTAMP | nullable |
| `valid_until` | TIMESTAMP | nullable |
| `confidence` | REAL | NOT NULL, CHECK 0.0–1.0 |
| `fact_status_id` | INTEGER | NOT NULL DEFAULT 1, FK → `fact_statuses(id)` |
| `inferred` | BOOLEAN | NOT NULL DEFAULT FALSE |
| `created_at` | TIMESTAMP | DEFAULT CURRENT_TIMESTAMP |
| `updated_at` | TIMESTAMP | DEFAULT CURRENT_TIMESTAMP |

Indexes: `subject_id`, `object_id`, `predicate_id`, `fact_status_id`, `valid_from + valid_until`.

### `fact_dependencies` table

Junction table with `ON DELETE RESTRICT` on both FKs (`parent_fact_id`, `child_fact_id` → `facts(id)`). This prevents SQLite from cascading deletes; deletion orchestration is handled in Rust (`forget/`) so that child inference chains can be re-evaluated before removal.

### `sources` table

One row per fact (plus any additional sources added via `add_source_to_fact`), linking the fact to its `SourceType` with optional `connector_instance_id` (FK to `connectors(id)`), `connector_type_id`, `raw_reference`, and `extraction_method_id`.

Unique constraint: `(fact_id, source_type_id, connector_instance_id, raw_reference)` (NULLs coerced via `COALESCE`).

### `fact_audit_log` table

Every insert, temporal update, status change, confidence change, source added, forget, and restore writes a row with:
- `change_type_id` → `change_types(id)` (`created`, `status_change`, `confidence_change`, `temporal_update`, `source_added`, `forgotten`, `restored`)
- `changed_by_id` → `changed_by_types(id)` (`user`, `system`, `inference_engine`, `nightly_optimization`)
- `old_value` / `new_value` — **column-only** JSON snapshots (e.g. `{"valid_until": "..."}`, not the full fact)
- `reason` — optional human-readable explanation

---

## Temporal Logic

When inserting a fact with the same `subject_id + predicate_id` as existing facts, the system evaluates temporal overlap:

1. **Non-overlapping ranges** — both facts remain `Active` (timeline).
2. **Old open-ended + new explicit start** — the old fact’s `valid_until` is closed at `now()`; the new fact is `Active`.
3. **Any other overlap** — the new fact is inserted as `Disputed`.

Overlap is checked with interval semantics (`[from, until)` where `None` = unbounded).

---

## Confidence

`src/confidence.rs` provides a placeholder model:

| SourceType | Initial Confidence |
|------------|-------------------|
| `UserEdit` | 1.00 |
| `Connector` | connector reliability score (default 0.80) |
| `Interaction` | 0.30 |
| `Import` | 0.80 |
| `Inference` | 0.00 (computed at insertion from parent confidences) |
| `System` | 1.00 |

`recalculate()` uses parent confidences × 0.8^depth × breadth factor. If the result falls below 0.20, the caller marks the fact `Disputed`. Every confidence recalculation writes a `confidence_change` audit entry. Full structural confidence is tracked in #51.

---

## Cascade Forget

`forget_fact()` performs a soft-delete:

1. JSON-serializes the fact + linked sources into `trash.payload`.
2. Inserts a `trash` row with 30-day `expires_at`.
3. Writes a `forgotten` audit log entry (`old_value` = full fact snapshot).
4. Manually removes all `fact_dependencies` rows where the fact is parent or child.
5. Hard-deletes the fact from `facts` (`sources` cascade; `fact_audit_log` persists).
6. For each former child:
   - If the child has zero remaining parents and `inferred = true`, recursively forget the child.
   - If the child has other parents, recalculate its confidence, update `facts.confidence`, and write a `confidence_change` audit entry.

`hard_delete_expired_trash()` removes trash rows whose `expires_at` has passed.

### Bulk forget matching

`forget_facts()` matches facts through `ForgetFilters` (fact id, predicate, subject, entity, source, `from`/`to` window). The two queries that consume the filters — `query_matching_fact_ids` (the id list for trashing) and `has_sensitive_match` (the sensitive-predicate safeguard) — share one `push_forget_filters` builder in `forget/trash.rs`, so a new filter field is added in exactly one place and the two queries cannot drift (issue #267).

`forget_connector_facts(instance_id)` trashes every fact a connector instance sourced (the connector-removal cascade), and `forget_connector_facts_by_raw_reference(instance_id, raw_references)` trashes only the facts that instance authored for the given `sources.raw_reference` values — the server-side-deletion (tombstone) path connectors use when their service reports a removed item (issue #247). The tombstone path removes only the matching `sources` rows and trashes a fact only when no sources remain, so a fact still corroborated by another connector or a non-connector source survives (PR #313 review). Both are idempotent, instance-scoped, and route through the same trash machinery; the `events.fact_id` FK cascade removes any events-subsystem overlay with the fact.

---

## Public API (`KnowledgeGraph`)

| Method | Description |
|--------|-------------|
| `insert_fact` | Insert with temporal + provenance handling |
| `get_fact` | Read by ID |
| `get_facts_by_subject` | List by subject entity |
| `get_facts_by_predicate` | List by predicate enum |
| `get_facts_by_object` | List by object entity |
| `get_active_facts_at` | Temporal point-in-time query |
| `update_fact_valid_until` | Close or extend a fact’s range (with audit) |
| `update_fact_status` | Lifecycle status change (with audit) |
| `forget_fact` | Soft-delete with cascade evaluation (with audit) |
| `get_audit_log` | Retrieve audit entries for a fact |
| `get_sources_for_fact` | Retrieve all sources for a fact |
| `add_source_to_fact` | Add a new source and write `source_added` audit entry |
| `query_audit_log` | Filtered audit log query across entities / predicates / time |
