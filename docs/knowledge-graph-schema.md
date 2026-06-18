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
| `preference_categories` | 7 | `PreferenceCategory` | `models::preference` |
| `preference_source_types` | 3 | `PreferenceSourceType` | `models::preference` |
| `predicates` | 11 | `Predicate` | `models::enums` |
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
| `preferences` | Learned user preferences with confidence, source_fact_id, and contextual lookup |
| `preference_contexts` | Normalized context conditions for preferences (no JSON) |
| `preference_sources` | Provenance for preference values |
| `preference_audit_log` | Immutable history of preference changes |
| `relationship_types` | Canonical relationship predicates (thin verbs); see Relationship Type DAG |
| `relationship_type_aliases` | Globally-unique English synonyms → canonical relationship type id |
| `relationship_type_hierarchy` | Parent/child edges between relationship types (vestigial — grouping lives in `categories`; see Category Aliases) |
| `categories` | Dewey-Decimal-style fact taxonomy with `memory_weight` |
| `fact_categories` | Many-to-many junction: facts ↔ categories (multi-tag precision + ranking) |
| `category_aliases` | Natural-language domain words → category id (see Category Aliases) |

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

The following 11 predicates are the complete seeded set:

1. `is_in`
2. `visited`
3. `owns`
4. `works_as`
5. `has_partner`
6. `has_parent`
7. `born_on`
8. `died_on`
9. `located_in`
10. `created_on`
11. `has_preference`

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
16. `031` — Category taxonomy (`categories`, `fact_categories`) + rename `predicates` → `relationship_types`
17. `035` — Relationship type DAG schema (`relationship_type_hierarchy`, `relationship_type_aliases`)
18. `036` — Seed relationship type aliases (self-aliases + legacy synonyms)
19. `037` — Seed remaining core predicates + self-aliases (#135); `ON CONFLICT` UPSERTs enforce the canonical `(id, name)` contract
20. `038` — `category_aliases` table + domain alias seed (#135); transactional with FK enforcement on, `IF NOT EXISTS` for idempotency

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

---

## Relationship Type DAG

Added in migration `035`:

- `relationship_type_hierarchy(child_id, parent_id)` — directed acyclic graph of relationship types. Multiple parents are allowed. Cycles are rejected in Rust before insert.
- `relationship_type_aliases(alias, relationship_type_id)` — English synonyms. `alias` is the primary key, so every alias resolves to exactly one canonical relationship type.

These tables let the agent discover relationship types instead of memorizing private names. Query traversal uses SQLite recursive CTEs:

```sql
WITH RECURSIVE descendants(id) AS (
  SELECT child_id FROM relationship_type_hierarchy WHERE parent_id = ?
  UNION
  SELECT h.child_id FROM relationship_type_hierarchy h
  JOIN descendants d ON h.parent_id = d.id
)
SELECT id FROM descendants;
```

### Subtree Fact Query

`queries::fact::get_facts_by_relationship_subtree(pool, subject_id, root_type_id, min_confidence, limit)` returns facts whose relationship type is `root_type_id` or any descendant. It walks the DAG in a single recursive CTE that **seeds with the root type itself** (so the root's own facts are included, unlike the descendants-only query above) and joins to `facts`:

```sql
WITH RECURSIVE subtree(id) AS (
    SELECT ?                       -- root_type_id
    UNION
    SELECT h.child_id FROM relationship_type_hierarchy h
    JOIN subtree s ON h.parent_id = s.id
)
SELECT f.*, e.name AS object_name
FROM facts f
JOIN subtree s ON f.relationship_type_id = s.id
LEFT JOIN entities e ON e.id = f.object_id
WHERE f.subject_id = ?
  AND f.pending_confirmation = 0
  AND f.fact_status_id NOT IN (5, 6)
  AND f.confidence >= ?
ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC
LIMIT ?;
```

The `UNION` (not `UNION ALL`) deduplicates relationship-type ids, so a type reachable via multiple hierarchy paths contributes each of its facts only once. Filters and ordering match `get_facts_by_subject_filtered` (non-pending, status `NOT IN (5, 6)` excluding Superseded and Forgotten, confidence floor, sorted by confidence). A matching `count_facts_by_relationship_subtree` counts the same set for pagination totals. The `KgQueryTool` exposes this via its `include_subtree` parameter (see `docs/kg-tools.md`).

Alias resolution is normalized (`trim`, lowercase, spaces → underscores) and cached in `RelationshipTypeCache`.

### Alias-Aware `ensure_relationship_type`

`ensure_relationship_type` is the single source of truth for resolving a relationship-type name to a canonical id:

1. Normalize the incoming name.
2. Query `relationship_type_aliases` for the normalized name. On hit, return the canonical `relationship_type_id`.
3. On miss, create a new canonical row in `relationship_types` and immediately register the normalized name as a self-alias.

Because every canonical name has a self-alias, the alias table is the only lookup source needed by both `ensure_relationship_type` and `get_relationship_type_id`.

### Collision Invariants

Canonical relationship type names and aliases share the same normalized namespace, so the following collisions are rejected before any write path persists data:

- A canonical name cannot be created if it normalizes to an existing alias.
- An alias cannot be created if it normalizes to an existing canonical name.

These checks are centralized in two helpers (`canonical_name_conflicts_with_alias` and `alias_conflicts_with_canonical_name`) that accept any `sqlx::Executor`, allowing the same invariant to run against both the connection pool and an open transaction. The explicit creation paths call the relevant check inside the same transaction as the insert:

- `insert_relationship_type`
- `insert_relationship_type_alias`

`ensure_relationship_type` resolves aliases first, so a name that matches an existing alias returns the canonical id rather than attempting to create a conflicting canonical type.

The extraction pipeline routes every fact's predicate through `ensure_relationship_type` (issue #136), so the `relationship_type_aliases` table is the sole source of truth for predicate canonicalization. The hardcoded `normalize_predicate` synonym map and the duplicate snake_case helper that previously lived in `mimir-knowledge/src/extract.rs` have been removed; all legacy synonyms are seeded as data by migrations `036`/`037`.

---

## Category Aliases & Subtree Retrieval

> Added in migrations `037`/`038` (Issue #135). This is the **category-first** ontology layer.

Mimir separates two concerns that previously risked overlapping:

- **Predicate canonicalization** lives on `relationship_type_aliases`. A predicate is a thin verb (`studied_at`, `works_at`, `has_partner`); English synonyms (`attended`, `employer`, `wife`) resolve to a single canonical id so the same verb is never stored under multiple rows. Migration `037` seeds the remaining core verbs (`studied`, `completed_degree`, `educational_status`, `job_title`, `likes`, `dislikes`) and their self-aliases via `ON CONFLICT` UPSERTs, so an upgrade corrects any stale row at a reserved id to the canonical mapping instead of silently preserving it.
- **Grouping / hierarchy / multi-tag precision** lives on the Dewey `categories` tree. A fact carries 1–3 category tags via `fact_categories`, and `categories.memory_weight` drives memory ranking (`confidence × memory_weight × temporal_boost × …`). Migration `038` adds `category_aliases`, which map natural-language domain words (`education`, `hobbies`, `residence`, `family`, `identity`, …) to a category id so callers can resolve a spoken word to a taxonomy node. It runs inside a transaction with foreign-key enforcement on and uses `CREATE … IF NOT EXISTS`, so a mid-run failure leaves no partial schema.

### Why categories, not a predicate hierarchy, for grouping

A predicate tree can only follow one axis (a predicate has a single canonical name and a parent path). Categories are many-to-many: "Alice works_at Foo as an engineer" can be both `510 Current Role` and `540 Skills & Expertise`, and "hobbies" spans `710 Music`, `740 Gaming`, `780 Outdoor Activities` — the granularity a reasoning agent needs (indoor vs outdoor for weather-aware suggestions, budget-relevant tags, shared-ground detection across two people). `relationship_type_hierarchy` therefore remains available but is **not seeded with abstract parent predicates**; grouping is done by category membership. Reworking `kg_query --include-subtree` (Issue #134) to expand by category subtree is a tracked follow-up; today it still expands by the (now intentionally sparse) predicate DAG.

### Retrieval API (`queries::category`)

- `resolve_category_alias(pool, alias) -> Option<i32>` — normalize (trim, lowercase, spaces→`_`) and look up `category_aliases`. Returns `None` for empty/unknown.
- `insert_category_alias(pool, alias, category_id)` — idempotent for the same alias→category mapping; rejects empty aliases (`Validation`), unknown category ids (`CategoryNotFound`), and rebinding an existing alias to a different category (`Validation`). Uses an atomic `INSERT OR IGNORE` + post-insert resolution so concurrent writers cannot surface a raw `UNIQUE`-constraint error; rebinds still return `Validation`.
- `get_descendant_category_ids(pool, root_id) -> Vec<i32>` — recursive CTE over `categories.parent_id` (root excluded).
- `get_facts_in_category_subtree(pool, root_id, limit)` — facts tagged anywhere in the subtree (root + descendants), reusing `get_facts_matching_any_categories`.
- `list_category_aliases(pool, category_id: Option<i32>)` — enumerate aliases (optionally filtered by category), returning `CategoryAlias` rows.

`KnowledgeGraph` exposes thin wrappers: `resolve_category_alias`, `insert_category_alias`, `get_descendant_category_ids`, `get_facts_in_category_subtree`.

### Seeded domain aliases (migration `038`)

| Alias | → Category | Dewey node |
|-------|-----------|------------|
| `education`, `schooling`, `academics`, `studies` | 550 | Education |
| `employment`, `career`, `job`, `work` | 510 | Current Role |
| `residence`, `housing`, `hometown`, `location` | 610 | Current Residence |
| `hobbies`, `interests`, `pastimes` / `leisure` | 770 / 700 | Collecting & Hobbies / Entertainment & Leisure |
| `pets`, `animals` | 440 | Pets & Animals |
| `family`, `relatives`, `kin` | 410 | Family |
| `identity`, `biography`, `profile` | 100 | Identity & Biography |

The six issue-#135 domains map to existing Dewey nodes rather than synthetic top-level parents; "personal" is intentionally spread across hobbies/leisure and pets, matching the Dewey design.
