# Fact Management

> **Scope:** `mimir-knowledge` crate — full fact CRUD, temporal overlap logic,
> confidence placeholder, cascade forget, and audit logging.
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

Junction table with `ON DELETE RESTRICT` on both FKs (`parent_fact_id`,
`child_fact_id` → `facts(id)`). This prevents SQLite from cascading deletes;
deletion orchestration is handled in Rust (`forget.rs`) so that child inference
chains can be re-evaluated before removal.

### `sources` table

One row per fact automatically inserted by `insert_fact`, linking the fact to
its `SourceType`.

### `fact_audit_log` table

Every insert, update (`valid_until`), status change, and delete writes a row with
JSON snapshots (`old_value`, `new_value`).

---

## Temporal Logic

When inserting a fact with the same `subject_id + predicate_id` as existing
facts, the system evaluates temporal overlap:

1. **Non-overlapping ranges** — both facts remain `Active` (timeline).
2. **Old open-ended + new explicit start** — the old fact’s `valid_until` is
   closed at `now()`; the new fact is `Active`.
3. **Any other overlap** — the new fact is inserted as `Disputed`.

Overlap is checked with interval semantics (`[from, until)` where `None` =
unbounded).

---

## Confidence

`src/confidence.rs` provides a placeholder model:

| SourceType | Initial Confidence |
|------------|-------------------|
| `UserEdit` | 1.00 |
| `Connector` | 0.80 |
| `Email`, `Calendar`, `Photo`, `Message` | 0.80 |
| `Inference` | 0.50 |

`recalculate()` averages remaining parent confidences × 0.8^depth. If the
result falls below 0.20, the caller marks the fact `Disputed`. Full structural
confidence (source weights, chain depth, transitive propagation) is tracked
in #51.

---

## Cascade Forget

`forget_fact()` performs a soft-delete:

1. JSON-serializes the fact + linked sources into `trash.payload`.
2. Inserts a `trash` row with 30-day `expires_at`.
3. Writes a `DELETE` audit log entry.
4. Manually removes `fact_dependencies` rows (RESTRICT FK).
5. Hard-deletes the fact from `facts` (`sources` and `fact_audit_log` cascade).
6. For each former child:
   - If the child has zero remaining parents and `inferred = true`, recursively
     forget the child.
   - If the child has other parents, recalculate its confidence and update
     `facts.confidence`.

`hard_delete_expired_trash()` removes trash rows whose `expires_at` has passed.

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
| `update_fact_valid_until` | Close or extend a fact’s range |
| `update_fact_status` | Lifecycle status change (with audit) |
| `forget_fact` | Soft-delete with cascade evaluation |
| `get_audit_log` | Retrieve audit entries for a fact |
