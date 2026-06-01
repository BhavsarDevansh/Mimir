# Preference System

> **Crate:** `mimir-knowledge`
> **Issue:** #53
> **Migration:** `023`

---

## Overview

The preference system is a behavioural index over the fact graph. Every preference row **must** reference a fact (`source_fact_id NOT NULL`). Preferences add only what facts cannot express: category, key, contextual conditions, confidence, and `overridden_by_user`.

Context is fully normalized into a separate table — no JSON anywhere in the preference schema.

---

## Design Decisions

1. **All preferences are facts.** Every preference creation requires a `source_fact_id`. The fact uses `Predicate::HasPreference` (ID 11).
2. **Caller provides `source_fact_id`.** The `upsert_preference` API accepts a `source_fact_id` directly. Callers create the fact first.
3. **No JSON in preferences.** `value` is plain `TEXT` (scalar string/bool/number as text). `context` is normalized into `preference_contexts(preference_id, key, value)` — one row per condition.
4. **Contextual lookup:** Query all preferences for `(entity_id, key)`, then count how many context rows match the query context map. Rank by match count descending. Tie-break: confidence desc, then `updated_at` desc. Fall back to preferences with zero context rows (the default).
5. **Uniqueness in application code:** Before insert, check if an existing preference has the same `(entity_id, key)` and an identical set of context rows. Reject if duplicate.
6. **Conflict resolution:**
   - Existing `overridden_by_user = true`, new inferred → rejected.
   - Existing inferred, new explicit → overwrite with audit log "overridden by user".
   - Both explicit → new wins (user updating their setting).
   - Both inferred → higher confidence wins; same confidence keeps existing.

---

## Schema

### Lookup Tables (re-seeded in migration 023)

| Table | Rows | Rust Enum | Variants |
|-------|------|-----------|----------|
| `preference_categories` | 7 | `PreferenceCategory` | CalendarBehavior, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, General |
| `preference_source_types` | 3 | `PreferenceSourceType` | Interaction, Fact, UserEdit |

### Core Tables

#### `preferences`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY AUTOINCREMENT | |
| `entity_id` | INTEGER | REFERENCES entities(id) — nullable for global preferences |
| `category_id` | INTEGER NOT NULL | REFERENCES preference_categories(id) |
| `key` | TEXT NOT NULL | |
| `value` | TEXT NOT NULL | scalar string/bool/number as text |
| `confidence` | REAL NOT NULL | CHECK (confidence >= 0.0 AND confidence <= 1.0) |
| `overridden_by_user` | BOOLEAN NOT NULL DEFAULT FALSE | |
| `source_fact_id` | INTEGER NOT NULL | REFERENCES facts(id) |
| `created_at` | TIMESTAMP | DEFAULT CURRENT_TIMESTAMP |
| `updated_at` | TIMESTAMP | DEFAULT CURRENT_TIMESTAMP |

Indexes: `idx_preferences_entity`, `idx_preferences_category`, `idx_preferences_key`.

#### `preference_contexts`

Normalized context conditions. No JSON.

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY AUTOINCREMENT | |
| `preference_id` | INTEGER NOT NULL | REFERENCES preferences(id) ON DELETE CASCADE |
| `context_key` | TEXT NOT NULL | |
| `context_value` | TEXT NOT NULL | |
| UNIQUE | `(preference_id, context_key)` | |

Index: `idx_preference_contexts_preference`.

#### `preference_sources`

Provenance for preferences.

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY AUTOINCREMENT | |
| `preference_id` | INTEGER NOT NULL | REFERENCES preferences(id) ON DELETE CASCADE |
| `source_type_id` | INTEGER NOT NULL | REFERENCES preference_source_types(id) |
| `source_id` | TEXT NOT NULL | opaque identifier (e.g. interaction UUID) |
| `extracted_at` | TIMESTAMP | DEFAULT CURRENT_TIMESTAMP |
| UNIQUE | `(preference_id, source_type_id, source_id)` | |

Index: `idx_preference_sources_preference`.

#### `preference_audit_log`

Immutable history. **No FK to `preferences(id)`** so history survives deletion (matches `fact_audit_log` pattern).

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY AUTOINCREMENT | |
| `preference_id` | INTEGER NOT NULL | |
| `change_type_id` | INTEGER NOT NULL | REFERENCES change_types(id) |
| `old_value` | TEXT | |
| `new_value` | TEXT | |
| `changed_at` | TIMESTAMP | DEFAULT CURRENT_TIMESTAMP |
| `changed_by_id` | INTEGER | REFERENCES changed_by_types(id) |
| `reason` | TEXT | |

Index: `idx_preference_audit_log_preference`.

---

## API

### `KnowledgeGraph` delegates

```rust
pub async fn insert_preference(
    &self,
    input: models::preference::UpsertPreferenceInput,
) -> Result<Preference, KnowledgeError>;

pub async fn upsert_preference(
    &self,
    input: models::preference::UpsertPreferenceInput,
) -> Result<(Preference, UpsertAction), KnowledgeError>;

pub async fn get_preference(
    &self,
    entity_id: Option<i32>,
    key: &str,
    query_context: &[(String, String)],
) -> Result<Option<Preference>, KnowledgeError>;

pub async fn get_preference_by_id(
    &self,
    id: i32,
) -> Result<Option<Preference>, KnowledgeError>;

pub async fn get_preference_contexts(
    &self,
    preference_id: i32,
) -> Result<Vec<PreferenceContext>, KnowledgeError>;

pub async fn get_preference_sources(
    &self,
    preference_id: i32,
) -> Result<Vec<PreferenceSource>, KnowledgeError>;

pub async fn get_preference_audit_log(
    &self,
    preference_id: i32,
) -> Result<Vec<PreferenceAuditLogEntry>, KnowledgeError>;
```

### Models

```rust
pub struct Preference {
    pub id: i32,
    pub entity_id: Option<i32>,
    pub category_id: i16,
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub overridden_by_user: bool,
    pub source_fact_id: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct UpsertPreferenceInput {
    pub preference: NewPreference,
    pub changed_by: ChangedBy,
    pub contexts: Vec<(String, String)>,
    pub sources: Vec<(PreferenceSourceType, String)>,
}

pub enum UpsertAction {
    Created,
    Overwritten,
    Rejected,
    KeptAsPrimary,
}
```

---

## Contextual Lookup Algorithm

1. Fetch all preferences for `(entity_id, key)`.
2. For each preference, fetch its context rows and count how many match entries in `query_context`.
3. Rank by match count descending. Tie-break: confidence desc, then `updated_at` desc.
4. If no preference has any matching context rows, return the preference with zero context rows (the default).
5. Return `None` if no rows at all.

---

## Conflict Resolution Rules

| Existing | New | Result | Audit Reason |
|----------|-----|--------|--------------|
| explicit | inferred | **Rejected** | — |
| inferred | explicit | **Overwritten** | "overridden by user" |
| explicit | explicit | **Overwritten** | "updated by user" |
| inferred | inferred (higher confidence) | **Overwritten** | "higher confidence inferred preference" |
| inferred | inferred (same/lower confidence) | **KeptAsPrimary** | — |

---

## Test Coverage

All tests live in `mimir-knowledge/tests/preference_tests.rs`:

1. Migration 023 creates tables and indexes correctly.
2. Insert roundtrip with contexts, sources, and audit log.
3. Duplicate rejection for identical `(entity_id, key, context_set)`.
4. Conflict resolution: explicit overrides inferred, higher-confidence wins, same-confidence keeps existing, user-override blocks inferred overwrites.
5. Contextual lookup: default fallback, specific wins over default, most-specific wins, no match returns None.
6. Audit logging on overwrite.
7. Source tracking returns all linked sources.
8. FK enforcement: non-existent `source_fact_id` fails.
9. Global preference with `NULL` entity_id.
