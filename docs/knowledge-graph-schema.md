# Knowledge Graph Schema

> **Crate:** `mimir-knowledge`  
> **Backend:** SQLite (single-file, local-first)  
> **File:** `~/.local/share/mimir/knowledge.db`

---

## Table Inventory

### Lookup Tables (Stable Integer IDs)

Lookup tables are seeded across migrations `001`, `012`, and `013` with stable integer IDs that map to Rust enums via `#[repr(i16)]` discriminants:

- Migration `001` seeds `entity_types` (7 variants), `entity_date_types`, `recurrence_types`, `location_types`, `fact_statuses`, `relation_types`, `source_types`, `preference_categories`, and `preference_source_types`.
- Migration `012` adds the `DateTime = 8` variant to `entity_types`.
- Migration `013` seeds `predicates` and `predicate_constraints`.

| Table | Rows | Rust Enum | Module |
|-------|------|-----------|--------|
| `entity_types` | 8 | `EntityType` | `models::entity` |
| `entity_date_types` | 6 | `EntityDateType` | `models::enums` |
| `recurrence_types` | 5 | `RecurrenceType` | `models::enums` |
| `location_types` | 5 | `LocationType` | `models::enums` |
| `fact_statuses` | 6 | `FactStatus` | `models::fact` |
| `relation_types` | 3 | `RelationType` | `models::enums` |
| `source_types` | 6 | `SourceType` | `models::source` |
| `preference_categories` | 5 | `PreferenceCategory` | `models::preference` |
| `preference_source_types` | 3 | `PreferenceSourceType` | `models::preference` |
| `predicates` | 10 | `Predicate` | `models::enums` |
| `extraction_methods` | 5 | `ExtractionMethod` | `models::source` |
| `change_types` | 7 | `ChangeType` | `models::audit_log` |
| `changed_by_types` | 4 | `ChangedBy` | `models::audit_log` |
| `connector_types` | 4 | `ConnectorType` | `models::enums` |

### Core Tables

| Table | Description |
|-------|-------------|
| `entities` | Graph nodes: people, places, events, objects, dates, etc. |
| `entity_aliases` | Alternative names for entities (dedup / search) |
| `entity_dates` | Temporal annotations (birth, anniversary, custom) with recurrence |
| `entity_locations` | Geographic / address data with validity windows |
| `facts` | Directed temporal edges between entities |
| `fact_dependencies` | Junction table linking inferred facts to parents |
| `sources` | Provenance for every fact (with `connector_id`, `connector_type_id`, `raw_reference`, `extraction_method_id`) |
| `preferences` | Learned user preferences with confidence |
| `preference_sources` | Provenance for preference values |

### System Tables

| Table | Description |
|-------|-------------|
| `system_state` | Key–value store for daemon state (e.g. condensed memory) |
| `fact_audit_log` | Immutable history with typed `change_type_id` and `changed_by_id`; column-only JSON snapshots |
| `dedup_queue` | Pending duplicate-fact resolutions |
| `entity_merge_queue` | Pending entity deduplication tasks |
| `trash` | Soft-deleted rows with full payload JSON |
| `entity_fts` | FTS5 virtual table for entity name / alias search |

### Predicate Taxonomy (New in 0.23.0)

Migration `013` introduces a controlled vocabulary for predicates:

- `predicates(id, name, description)` — canonical predicate names with stable IDs.
- `predicate_constraints(predicate_id, allowed_subject_type_id, allowed_object_type_id)` — valid subject/object type combinations per predicate.

Seeded predicates: `is_in`, `visited`, `owns`, `works_as`, `has_partner`, `has_parent`, `born_on`, `died_on`, `located_in`, `created_on`.

Validation is enforced at fact-insert time via `validate_predicate(subject_type, predicate, object_type)`.

---

## Enum ↔ Lookup Mapping

Every enum variant has an explicit discriminant matching its DB seed ID:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[repr(i16)]
pub enum EntityType {
    Person = 1,
    Place = 2,
    Event = 3,
    Object = 4,
    Concept = 5,
    Organization = 6,
    Activity = 7,
    DateTime = 8,
}
```

`static_assertions::const_assert!` verifies:
- No zero-ID variants where required.
- Variant count matches expected seed count.

Runtime tests (`lookup_sync_test.rs`) query every lookup table and assert bidirectional sync: every enum variant has a DB row and every DB row has an enum variant.

---

## ID Size Choices

| Domain | Type | Rationale |
|--------|------|-----------|
| Lookup table IDs | `i16` | Max 32 767 — sufficient for all taxonomies |
| Entity / fact / preference IDs | `i32` | Max ~2 billion — sufficient for lifetime facts |
| Foreign key columns | Match referenced table | Consistent size, no casts |

---

## Migration Ordering Rationale

Migrations are strictly ordered by foreign-key dependencies:

1. `001` — Lookup tables + `system_state` (no FKs)
2. `002` — `entities` (depends on `entity_types`)
3. `003` — `entity_dates` (depends on `entities`, `entity_date_types`, `recurrence_types`)
4. `004` — `entity_locations` (depends on `entities`, `location_types`)
5. `005` — `facts` (depends on `entities`, `fact_statuses`)
6. `006` — `fact_dependencies` (depends on `facts`, `relation_types`)
7. `007` — `sources` (depends on `facts`, `source_types`)
8. `008` — `preferences` + `preference_sources` (depends on `preference_categories`, `preference_source_types`)
9. `009` — Audit log + queues (depends on `facts`, `entities`)
10. `010` — `trash` (standalone, no FKs)
11. `011` — FTS5 virtual table + triggers (depends on `entities`)
12. `012` — `DateTime` entity type seed
13. `013` — Predicate taxonomy tables + constraints
14. `021` — Additional source types (`CasualMention`, `Import`, `System`)
15. `022` — Provenance audit refactor: remap `source_types` to 6 variants, add `extraction_methods` / `change_types` / `changed_by_types`, recreate `sources` and `fact_audit_log` with typed FKs

---

## SQLite Configuration

- **Journal mode:** WAL (write-ahead logging) for concurrency and durability.
- **Foreign keys:** Enabled on every connection.
- **Max connections:** 5 (SQLite single-writer; pool smooths async access).

---

## Full-Text Search

`entity_fts` is an FTS5 virtual table shadowing `entities`. Triggers on `entities` keep the index in sync automatically on insert, update, and delete.

Search flow (`get_by_name`):
1. Exact name match (step 1)
2. Exact alias match (step 2)
3. FTS5 fuzzy search with rank threshold ≥ 0.8 (step 3)

Results are deduplicated, scored, and capped at 10.

---

## Entity Deduplication

Two-phase dedup implemented in Rust:

1. **Exact-match auto-merge** — case-insensitive name match; survivor is the entity with more facts. Merged aliases are preserved; facts are repointed via FK update.
2. **Overlapping-alias flagging** — shared alias strings across different entities insert rows into `entity_merge_queue` with `Pending` status for human review.
3. **LLM semantic dedup** — stubbed in #49; full implementation deferred to Phase 2 optimization (#50+).

---

## Entity Dates & Recurrence

`entity_dates` stores ISO-8601 date/datetime values with a recurrence type:

- `None` — one-time date.
- `Daily` — every day.
- `Weekly` — same weekday each week.
- `Monthly` — same day each month (falls back to last valid day).
- `Yearly` — anniversary; Feb 29 falls back to Mar 1 in non-leap years.

All recurrence math is UTC-internal; timezone formatting is a presentation-layer concern.

---

## Future Work

- Inference engine (`inference/`) — Rust-native transitivity, contradiction, threshold rules.
- Optimization pipeline (`optimization/`) — Nightly dedup, confidence recalc, dormant cleanup.
- Fact extraction (`extract.rs`) — LLM-assisted structured extraction with Rust validation.
