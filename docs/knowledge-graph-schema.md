# Knowledge Graph Schema

> **Crate:** `mimir-knowledge`
>
> **Backend:** SQLite (single-file, local-first)
>
> **File:** `~/.local/share/mimir/knowledge.db`

---

## Table Inventory

### Lookup Tables (Stable Integer IDs)

Lookup tables are seeded across migrations `001`, `012`, `013`, `020`, `022`, `023`, `024`, `032`, `039`, and `046` with stable integer IDs that map to Rust enums via `#[repr(i16)]` discriminants:

- Migration `001` seeds `entity_types` (7 variants), `recurrence_types`, `location_types`, `fact_statuses`, `relation_types`, `source_types`, `preference_categories`, and `preference_source_types`. (`entity_date_types` was also seeded here but is dropped in migration `040` — see Events & Reminders.)
- Migration `012` adds the `DateTime = 8` variant to `entity_types`.
- Migration `013` seeds `predicates` and `predicate_constraints` (renamed to `relationship_types` / `relationship_constraints` by migration `031`).
- Migration `023` re-seeds `preference_categories` (7 rows: CalendarBehavior, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, General) and `preference_source_types` (3 rows: Interaction, Fact, UserEdit).
- Migration `024` adds the `Contradicts = 4` variant to `relation_types`.
- Migration `039` seeds the events overlay lookups: `event_types`, `event_statuses`, and `auto_complete_policies`.
- The enum conversions in `mimir-knowledge` align the lookup identifiers across storage and the API/tool contracts: `ChangeType` / `ChangedBy` (`models::audit_log`) and `EntityType` (`models::entity`) expose `as_str()` + `TryFrom<i16>` (plus case-insensitive `FromStr` where input parsing exists), and the KB route / `kg_*` tool helpers delegate to them instead of re-typing the name tables (issue #358). Alignment is by stable identifier — `TryFrom<i16>` keeps the lookup rows and enum discriminants in lock-step — while endpoint string representations may differ: audit SQL responses report `changed_by` as the lowercase lookup name (`user`), whereas fact-detail output uses the title-case variant string (`User`).

| Table | Rows | Rust Enum | Module |
|-------|------|-----------|--------|
| `entity_types` | 8 | `EntityType` | `models::entity` |
| `recurrence_types` | 5 | `RecurrenceType` | `models::enums` |
| `location_types` | 6 | `LocationType` | `models::enums` |
| `fact_statuses` | 6 | `FactStatus` | `models::fact` |
| `relation_types` | 4 | `RelationType` | `models::enums` |
| `source_types` | 6 | `SourceType` | `models::source` |
| `preference_categories` | 7 | `PreferenceCategory` | `models::preference` |
| `preference_source_types` | 3 | `PreferenceSourceType` | `models::preference` |
| `extraction_methods` | 5 | `ExtractionMethod` | `models::source` |
| `change_types` | 9 | `ChangeType` | `models::audit_log` |
| `changed_by_types` | 4 | `ChangedBy` | `models::audit_log` |
| `connector_types` | 4 | `ConnectorType` | `models::enums` |
| `event_types` | 6 | `EventType` | `models::enums` |
| `event_statuses` | 5 | `EventStatus` | `models::enums` |
| `auto_complete_policies` | 3 | `AutoCompletePolicy` | `models::enums` |
| `memory_priorities` | 4 | `MemoryPriority` | `models::memory` |

### Core Tables

| Table | Description |
|-------|-------------|
| `entities` | Graph nodes: people, places, events, objects, dates, etc. |
| `entity_aliases` | Alternative names for entities (dedup / search) |
| `entity_locations` | Geographic / address data with validity windows; `source_fact_id` links a row to the fact that produced it (Phase 3 S3 / #193) |
| `facts` | Directed temporal edges between entities |
| `events` | Lifecycle + recurrence overlay on facts (trigger date, recurrence, status, auto-complete policy); see Events & Reminders |
| `fact_dependencies` | Junction table linking inferred facts to parents |
| `sources` | Provenance for every fact (with `connector_instance_id` FK to `connectors(id)`, `connector_type_id`, `raw_reference`, `extraction_method_id`) |
| `preferences` | Learned user preferences with confidence, source_fact_id, and contextual lookup |
| `preference_contexts` | Normalized context conditions for preferences (no JSON) |
| `preference_sources` | Provenance for preference values |
| `preference_audit_log` | Immutable history of preference changes |
| `relationship_types` | Canonical relationship predicates (thin verbs); see Relationship Type DAG |
| `relationship_type_aliases` | Globally-unique English synonyms → canonical relationship type id |
| `relationship_type_hierarchy` | Parent/child edges between relationship types (vestigial — grouping lives in `categories`; see Category Aliases) |
| `relationship_constraints` | Valid subject/object entity-type combinations per relationship type (renamed from `predicate_constraints` by migration `031`) |
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
| `pending_event_meta` | Fact-keyed cache of the extracted event shape for pending sensitive facts; consumed on confirm, cascade-deleted on reject (migration 041) |
| `pending_location_meta` | Fact-keyed cache of the extracted `NormalizedLocation` shape for pending sensitive "where" facts; consumed on confirm, cascade-deleted on reject (migration 048) |
| `entity_fts` | FTS5 virtual table for entity name / alias search |
| `optimization_runs` | One row per nightly-optimization run (started/finished, status, trigger) — migration `030` |
| `optimization_pass_runs` | One row per pass within a run (pass name, status, counts, error) — migration `030` |

### Predicate Taxonomy

Migration `013` introduced a controlled vocabulary for predicates as `predicates(id, name, description)` and `predicate_constraints(predicate_id, allowed_subject_type_id, allowed_object_type_id)`. Migration `031` renamed these to **`relationship_types`** and **`relationship_constraints`** (and dropped the old `predicates` / `Predicate`-enum mapping); the canonical predicate names now live in `relationship_types`, and `relationship_constraints` holds the valid subject/object type combinations per predicate.

The original 11 seeded predicates (`is_in`, `visited`, `owns`, `works_as`, `has_partner`, `has_parent`, `born_on`, `died_on`, `located_in`, `created_on`, `has_preference`) were carried over, and migration `025` added `rejected_action` (id 12) for the threshold inference rule. The full set is data-driven and extensible — see [Relationship Type DAG](#relationship-type-dag) and [Category Aliases & Subtree Retrieval](#category-aliases--subtree-retrieval) for the alias and hierarchy layer.

Validation is enforced at fact-insert time via `validate_predicate(subject_type, predicate, object_type)`, which queries `relationship_constraints`.

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
3. `003` — `entity_dates` (depends on `entities`, `entity_date_types`, `recurrence_types`) — **dropped by migration `040`**; superseded by the events overlay (migration `039`).
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
21. `023` — Preference system refactor (#53): normalized context, `source_fact_id`, contextual lookup; breaking recreate of `preferences` / `preference_sources`
22. `024` — `Contradicts` relation type (id 4) for the contradiction rule (#54)
23. `025` — `rejected_action` predicate (id 12) for the threshold rule (#54)
24. `026` — `facts.pending_confirmation` flag + partial index for sensitive facts
25. `027` — `rejected` change type (id 8) for explicit sensitive-fact rejection
26. `028` — Composite performance index for `kg_query` / `kg_related` / `kg_search`
27. `029` — `predicates.sensitive` flag + seed of medical/financial/identity predicates (bulk-forget safeguard)
28. `030` — `optimization_runs` table for nightly-optimization run tracking
29. `032` — `memory_priorities` lookup for the memory-ranking engine
30. `033` — `relationship_types.default_memory_priority_id` with per-predicate defaults
31. `034` — `content_update` change type (id 9) for object-literal edits
32. `039` — Events & reminders overlay: `event_types`, `event_statuses`, `auto_complete_policies`, `events` (#74)
33. `040` — Drop superseded `entity_dates` / `entity_date_types` (#74)
34. `041` — `pending_event_meta` cache for sensitive-fact event shape across the confirmation boundary (#74)
35. `042` — Connector instance registry: `connectors` + `connector_statuses` / `connector_auth_states` lookups (#179)
36. `043` — `sources.connector_instance_id` FK migration (`connector_id TEXT` → integer FK to `connectors(id)`) (#180)
37. `044` — `entity_locations.source_fact_id` FK for the location-overlay write path (#193)
38. `045` — `entity_locations` coordinate index for proximity queries (#194)
39. `046` — `Geographic` location type (id 6) for place coordinate anchoring (#196)
40. `047` — Partial unique index on `entity_locations(entity_id)` scoped to `location_type_id = 6` (single `Geographic` row per place) (#196)
41. `048` — `pending_location_meta` cache for sensitive-fact location shape across the confirmation boundary (#226)
42. `049` — `connectors.durable_state` column: opaque, connector-owned durable state persisted by the supervisor (the Email connector's bounded LLM-extraction retry ledger) (#262)

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

### Entity resolution (`get_by_name_typed` + `resolve_entity`)

The ingestion pipeline resolves a name to an entity before inserting a fact (`mimir-knowledge::normalize::resolve_entity`, Phase 3 F5 / #182). It reuses the same three-stage search but restricts candidates to the requested `EntityType` via `get_by_name_typed`, then applies a resolution policy (`pick_resolution`):

1. An exact-name or exact-alias hit always resolves to the existing entity.
2. A fuzzy hit resolves only when its normalised score is ≥ `FUZZY_RESOLVE_THRESHOLD` (`0.9`); a weaker fuzzy match is treated as a miss.
3. If no candidate resolves, a new entity is created with the requested type.

Cross-type matches are dropped, so "Apple" resolved as a `Concept` never merges into the `Organization` "Apple Inc". The untyped `get_by_name` remains the general-purpose search surface (kg_search, auto-merge). Alias creation is not auto-learned from fuzzy matches; it stays explicit via `preferred_name`.

---

## Entity Deduplication

Two-phase dedup implemented in Rust:

1. **Exact-match auto-merge** — case-insensitive name match; survivor is the entity with more facts. Merged aliases are preserved; facts are repointed via FK update.
2. **Overlapping-alias flagging** — shared alias strings across different entities insert rows into `entity_merge_queue` with `Pending` status for human review.
3. **LLM semantic dedup** — stubbed in #49; full implementation deferred to Phase 2 optimization (#50+).

---

## Events & Reminders

The `entity_dates` subsystem was superseded and dropped in migration `040` (issue #74). Temporal lifecycle is now a **recurrence + lifecycle overlay on facts**, implemented in migration `039`.

A fact whose `valid_from` lies in the future is a one-time event; a fact tagged with recurrence (e.g. a birthday) is a recurring event; a fact with `requires_user_action` is a task. The `events` table attaches one overlay row per fact (`fact_id` is `UNIQUE`, `ON DELETE CASCADE`):

- `trigger_date` — when the event next surfaces.
- `recurrence_type_id` → `RecurrenceType` (`None`, `Daily`, `Weekly`, `Monthly`, `Yearly`). Feb 29 falls back to Mar 1 in non-leap years.
- `event_type_id` → `EventType` (`birthday`, `appointment`, `deadline`, `task`, `reminder`, `custom`).
- `status_id` → `EventStatus` (`Pending`, `Active`, `Completed`, `Dismissed`, `Snoozed`).
- `auto_complete_policy_id` → `AutoCompletePolicy` (`AutoCompleteOnDate`, `RequiresUserAction`, `Recurring`).
- `requires_user_action` — marks tasks that need explicit user action.

The `events.upcoming_scan` job (default 06:00 & 18:00) derives overlays, auto-completes past one-time events, and advances recurring events. Source facts surface in the "Upcoming" memory section directly; the overlay manages lifecycle status and recurrence advancement only. `pending_event_meta` (migration `041`) preserves the derived event shape for sensitive facts across the confirmation boundary so that `confirm_fact` can rebuild the overlay faithfully.

All recurrence math is UTC-internal; timezone formatting is a presentation-layer concern. See [Events & Reminders](events-reminders.md).

---

## Implemented Subsystems

The subsystems below are all implemented and documented separately:

- Inference engine (`inference/`) — Rust-native transitivity, contradiction, and threshold rules. See [Inference Engine](inference-engine.md).
- Optimization pipeline (`optimization/`) — Nightly dedup, confidence recalc, dormant cleanup, and compaction. See [Nightly Optimization](nightly-optimization.md).
- Fact extraction (`extract.rs`) — LLM-orchestrated structured extraction with Rust validation. See [Fact Extraction Pipeline](fact-extraction-pipeline.md).
- Confidence model — structural, graph-derived, no LLM, no decay. See [Confidence Model](Confidence-Model.md).

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

The extraction pipeline routes every fact's predicate through `ensure_relationship_type` (issue #136), so the `relationship_type_aliases` table is the sole source of truth for predicate canonicalization. The hardcoded `normalize_predicate` synonym map and the duplicate snake_case helper that previously lived in `mimir-knowledge/src/extract/` have been removed; all legacy synonyms are seeded as data by migrations `036`/`037`.

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
