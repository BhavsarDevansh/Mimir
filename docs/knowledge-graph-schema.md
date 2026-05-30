# Knowledge Graph Schema

> **Crate:** `mimir-knowledge`  
> **Backend:** SQLite (single-file, local-first)  
> **File:** `~/.local/share/mimir/knowledge.db`

---

## Table Inventory

### Lookup Tables (Stable Integer IDs)

All lookup tables are seeded in migration `001` with stable integer IDs that map to Rust enums via `#[repr(i16)]` discriminants.

| Table | Rows | Rust Enum | Module |
|-------|------|-----------|--------|
| `entity_types` | 7 | `EntityType` | `models::entity` |
| `entity_date_types` | 6 | `EntityDateType` | `models::enums` |
| `recurrence_types` | 5 | `RecurrenceType` | `models::enums` |
| `location_types` | 5 | `LocationType` | `models::enums` |
| `fact_statuses` | 6 | `FactStatus` | `models::fact` |
| `relation_types` | 3 | `RelationType` | `models::enums` |
| `source_types` | 7 | `SourceType` | `models::source` |
| `preference_categories` | 5 | `PreferenceCategory` | `models::preference` |
| `preference_source_types` | 3 | `PreferenceSourceType` | `models::preference` |

### Core Tables

| Table | Description |
|-------|-------------|
| `entities` | Graph nodes: people, places, events, objects, etc. |
| `entity_aliases` | Alternative names for entities (dedup / search) |
| `entity_dates` | Temporal annotations (birth, anniversary, custom) |
| `entity_locations` | Geographic / address data with validity windows |
| `facts` | Directed temporal edges between entities |
| `fact_dependencies` | Junction table linking inferred facts to parents |
| `sources` | Provenance for every fact |
| `preferences` | Learned user preferences with confidence |
| `preference_sources` | Provenance for preference values |

### System Tables

| Table | Description |
|-------|-------------|
| `system_state` | Key–value store for daemon state (e.g. condensed memory) |
| `fact_audit_log` | Immutable history of fact insert / update / delete |
| `dedup_queue` | Pending duplicate-fact resolutions |
| `entity_merge_queue` | Pending entity deduplication tasks |
| `trash` | Soft-deleted rows with full payload JSON |
| `entity_fts` | FTS5 virtual table for entity name / alias search |

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

---

## SQLite Configuration

- **Journal mode:** WAL (write-ahead logging) for concurrency and durability.
- **Foreign keys:** Enabled on every connection.
- **Max connections:** 5 (SQLite single-writer; pool smooths async access).

---

## Full-Text Search

`entity_fts` is an FTS5 virtual table shadowing `entities`. Triggers on `entities` keep the index in sync automatically on insert, update, and delete.

---

## Future Work

- Query builders (`queries/` modules) — CRUD, temporal retrieval, graph traversal.
- Inference engine (`inference/`) — Rust-native transitivity, contradiction, threshold rules.
- Optimization pipeline (`optimization/`) — Nightly dedup, confidence recalc, dormant cleanup.
- Fact extraction (`extract.rs`) — LLM-assisted structured extraction with Rust validation.
