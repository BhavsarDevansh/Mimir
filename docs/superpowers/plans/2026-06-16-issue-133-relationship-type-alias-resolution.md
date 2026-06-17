# Issue #133 — Relationship Type Alias Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ensure_relationship_type` the single source of truth for resolving relationship-type names to canonical IDs, backed by the `relationship_type_aliases` table, and deprecate the hardcoded predicate map in `extract.rs`.

**Architecture:** All relationship-type name resolution routes through the alias table. On cache/DB miss, a new canonical type is created and its normalized name is registered as its own alias. A new migration seeds existing canonical names as self-aliases plus legacy hardcoded synonyms. The deprecated `normalize_predicate` map is retained as a fallback but removed from the active extraction path.

**Tech Stack:** Rust, SQLite, sqlx, tokio.

---

## Task 1: Failing Tests

**Files:**
- Modify: `mimir-knowledge/tests/relationship_type_dag_test.rs`

- [ ] **Step 1: Add failing test for alias resolution in `ensure_relationship_type`**

```rust
#[tokio::test]
async fn ensure_relationship_type_resolves_alias_to_canonical() {
    let (_dir, kg) = setup().await;
    let canonical_id = kg.ensure_relationship_type("studied_at").await.unwrap();
    kg.insert_relationship_type_alias("attended", canonical_id)
        .await
        .unwrap();

    let resolved_id = kg.ensure_relationship_type("attended").await.unwrap();
    assert_eq!(resolved_id, canonical_id);
    assert_eq!(
        kg.get_relationship_type_id("attended").await.unwrap(),
        Some(canonical_id)
    );
}
```

- [ ] **Step 2: Add failing test for new type creation + self-alias**

```rust
#[tokio::test]
async fn ensure_relationship_type_creates_new_type_and_self_alias() {
    let (_dir, kg) = setup().await;
    let id = kg.ensure_relationship_type("foo_bar").await.unwrap();

    let name_id = kg.get_relationship_type_id("foo_bar").await.unwrap();
    assert_eq!(name_id, Some(id));

    let aliases: Vec<String> = sqlx::query_scalar(
        "SELECT alias FROM relationship_type_aliases WHERE relationship_type_id = ?",
    )
    .bind(id)
    .fetch_all(&kg.pool())
    .await
    .unwrap();
    assert!(aliases.contains(&"foo_bar".to_string()));
}
```

- [ ] **Step 3: Run tests to confirm they fail**

Run: `cargo test --package mimir-knowledge --test relationship_type_dag_test -- ensure_relationship_type_resolves_alias_to_canonical ensure_relationship_type_creates_new_type_and_self_alias -v`
Expected: FAIL — alias not resolved, self-alias not created.

- [ ] **Step 4: Commit**

```bash
git add mimir-knowledge/tests/relationship_type_dag_test.rs
git commit -m "test: add failing tests for relationship type alias resolution (#133)"
```

---

## Task 2: Migration to Seed Relationship Type Aliases

**Files:**
- Create: `mimir-knowledge/src/db/migrations/036_seed_relationship_type_aliases.sql`

- [ ] **Step 1: Write the migration**

```sql
-- no-transaction
-- ============================================================================
-- 036: Seed relationship type aliases: self-aliases + legacy hardcoded synonyms
-- ============================================================================
PRAGMA foreign_keys = OFF;

-- 1. Self-aliases for every existing canonical relationship type.
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types;

-- 2. Legacy hardcoded synonyms from extract.rs::normalize_predicate.
--    Each resolves to a canonical relationship type already in relationship_types.
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'attended', id FROM relationship_types WHERE name = 'studied_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'went_to', id FROM relationship_types WHERE name = 'studied_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'graduated_from', id FROM relationship_types WHERE name = 'studied_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'alumni_of', id FROM relationship_types WHERE name = 'studied_at';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'hobbies', id FROM relationship_types WHERE name = 'hobby';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'interests', id FROM relationship_types WHERE name = 'hobby';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'works_for', id FROM relationship_types WHERE name = 'works_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'employer', id FROM relationship_types WHERE name = 'works_at';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'profession', id FROM relationship_types WHERE name = 'works_as';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'occupation', id FROM relationship_types WHERE name = 'works_as';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'resides_in', id FROM relationship_types WHERE name = 'based_in';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'current_city', id FROM relationship_types WHERE name = 'based_in';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'previously_lived_in', id FROM relationship_types WHERE name = 'lived_in';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'former_city', id FROM relationship_types WHERE name = 'lived_in';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'pet', id FROM relationship_types WHERE name = 'has_pets';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'pets', id FROM relationship_types WHERE name = 'has_pets';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'owns_pet', id FROM relationship_types WHERE name = 'has_pets';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'brother', id FROM relationship_types WHERE name = 'has_sibling';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'sister', id FROM relationship_types WHERE name = 'has_sibling';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'siblings', id FROM relationship_types WHERE name = 'has_sibling';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'spouse', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'boyfriend', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'girlfriend', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'partner', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'wife', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'husband', id FROM relationship_types WHERE name = 'has_partner';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'father', id FROM relationship_types WHERE name = 'has_parent';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'mother', id FROM relationship_types WHERE name = 'has_parent';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'parents', id FROM relationship_types WHERE name = 'has_parent';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'son', id FROM relationship_types WHERE name = 'has_child';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'daughter', id FROM relationship_types WHERE name = 'has_child';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'children', id FROM relationship_types WHERE name = 'has_child';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'nickname', id FROM relationship_types WHERE name = 'preferred_name';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'nick_name', id FROM relationship_types WHERE name = 'preferred_name';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'called', id FROM relationship_types WHERE name = 'preferred_name';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'goes_by', id FROM relationship_types WHERE name = 'preferred_name';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favorite_food', id FROM relationship_types WHERE name = 'favourite_food';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'fav_food', id FROM relationship_types WHERE name = 'favourite_food';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favourite_food', id FROM relationship_types WHERE name = 'favourite_food';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favorite_colour', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favorite_color', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'fav_color', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'fav_colour', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'color', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'colour', id FROM relationship_types WHERE name = 'favourite_colour';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'food_allergy', id FROM relationship_types WHERE name = 'health_condition';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'medical_condition', id FROM relationship_types WHERE name = 'health_condition';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'condition', id FROM relationship_types WHERE name = 'health_condition';

PRAGMA foreign_keys = ON;
```

- [ ] **Step 2: Verify migration runs in tests**

Run: `cargo test --package mimir-knowledge --test migrations_test`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add mimir-knowledge/src/db/migrations/036_seed_relationship_type_aliases.sql
git commit -m "feat: seed relationship type aliases with self-aliases and legacy synonyms (#133)"
```

---

## Task 3: Refactor `ensure_relationship_type` to Resolve Aliases

**Files:**
- Modify: `mimir-knowledge/src/lib.rs`

- [ ] **Step 1: Add a private helper that resolves or creates inside a transaction**

Replace the body of `ensure_relationship_type_in_tx` with:

```rust
pub(crate) async fn ensure_relationship_type_in_tx(
    &self,
    tx: &mut sqlx::SqliteTransaction<'_>,
    name: &str,
) -> Result<i16, KnowledgeError> {
    let Some(normalized) = normalize_relationship_alias(name) else {
        return Err(KnowledgeError::Validation(
            "relationship type name cannot be empty".to_string(),
        ));
    };

    // 1. In-memory cache.
    {
        let cache = self.relationship_type_cache.read().await;
        if let Some(&id) = cache.alias_to_id.get(&normalized) {
            return Ok(id);
        }
    }

    // 2. Alias table is the single source of truth.
    let row: Option<(i16,)> = sqlx::query_as(
        "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
    )
    .bind(&normalized)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some((id,)) = row {
        let mut cache = self.relationship_type_cache.write().await;
        cache.alias_to_id.insert(normalized.clone(), id);
        cache.name_to_id.insert(normalized, id);
        return Ok(id);
    }

    // 3. Alias miss: create new canonical type, then register self-alias.
    if canonical_name_conflicts_with_alias(&mut **tx, &normalized).await? {
        return Err(KnowledgeError::Validation(format!(
            "relationship type name '{}' conflicts with an existing alias",
            normalized
        )));
    }

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO relationship_types (name, description) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET name = relationship_types.name RETURNING id",
    )
    .bind(&normalized)
    .bind(format!("Auto-created relationship_type: {}", normalized))
    .fetch_one(&mut **tx)
    .await?;
    let id = id as i16;

    sqlx::query(
        "INSERT INTO relationship_type_aliases (alias, relationship_type_id) VALUES (?, ?)",
    )
    .bind(&normalized)
    .bind(id)
    .execute(&mut **tx)
    .await?;

    let mut cache = self.relationship_type_cache.write().await;
    cache.name_to_id.insert(normalized.clone(), id);
    cache.alias_to_id.insert(normalized, id);
    Ok(id)
}
```

Then make `ensure_relationship_type` delegate:

```rust
pub async fn ensure_relationship_type(&self, name: &str) -> Result<i16, KnowledgeError> {
    let mut tx = self.pool.begin().await?;
    let id = self.ensure_relationship_type_in_tx(&mut tx, name).await?;
    tx.commit().await?;
    Ok(id)
}
```

- [ ] **Step 2: Update `get_relationship_type_id` to resolve aliases**

```rust
pub async fn get_relationship_type_id(
    &self,
    name: &str,
) -> Result<Option<i16>, KnowledgeError> {
    let Some(normalized) = normalize_relationship_alias(name) else {
        return Ok(None);
    };

    {
        let cache = self.relationship_type_cache.read().await;
        if let Some(&id) = cache.alias_to_id.get(&normalized) {
            return Ok(Some(id));
        }
    }

    let row: Option<(i16,)> = sqlx::query_as(
        "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
    )
    .bind(&normalized)
    .fetch_optional(&self.pool)
    .await?;

    if let Some((id,)) = row {
        let mut cache = self.relationship_type_cache.write().await;
        cache.alias_to_id.insert(normalized, id);
        Ok(Some(id))
    } else {
        Ok(None)
    }
}
```

- [ ] **Step 3: Run targeted tests**

Run: `cargo test --package mimir-knowledge --test relationship_type_dag_test`
Expected: PASS.

- [ ] **Step 4: Run knowledge-graph package tests**

Run: `cargo test --package mimir-knowledge`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add mimir-knowledge/src/lib.rs
git commit -m "feat: make ensure_relationship_type alias-aware (#133)"
```

---

## Task 4: Deprecate Hardcoded Predicate Map in Extraction

**Files:**
- Modify: `mimir-knowledge/src/extract.rs`

- [ ] **Step 1: Replace extraction call with direct normalization**

In `normalize_and_expand_facts`:

```rust
async fn normalize_and_expand_facts(
    kg: &KnowledgeGraph,
    facts: Vec<ExtractedFact>,
) -> Result<Vec<ExtractedFact>, KnowledgeError> {
    let mut result = Vec::new();
    for mut fact in facts {
        fact.relationship_type = normalize_relationship_type(&fact.relationship_type);
        result.extend(split_list_objects(&fact));
    }
    Ok(result)
}

fn normalize_relationship_type(pred: &str) -> String {
    pred.trim().to_lowercase().replace(' ', "_")
}
```

- [ ] **Step 2: Mark `normalize_predicate` as deprecated fallback**

Add a doc comment and deprecation attribute:

```rust
/// Deprecated fallback synonym map for relationship types.
///
/// Relationship type aliases are now the single source of truth via
/// [`KnowledgeGraph::ensure_relationship_type`]. This hardcoded map is retained
/// only as a fallback until the core ontology is fully seeded.
#[allow(dead_code)]
#[deprecated(
    note = "Relationship type aliases are now the single source of truth via ensure_relationship_type; this hardcoded map is kept only as a fallback until #132 fully seeds the ontology"
)]
async fn normalize_predicate(kg: &KnowledgeGraph, pred: &str) -> Result<String, KnowledgeError> {
    // existing body minus alias-table branch
}
```

- [ ] **Step 3: Run extraction tests**

Run: `cargo test --package mimir-knowledge --test extraction_test`
Expected: PASS.

- [ ] **Step 4: Run full workspace tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add mimir-knowledge/src/extract.rs
git commit -m "refactor: deprecate hardcoded predicate map in extraction (#133)"
```

---

## Task 5: Documentation & Versioning

**Files:**
- Modify: `Cargo.toml`, `CHANGELOG.md`, `docs/wiki/what-works-now.md`
- Modify: `docs/knowledge-graph-schema.md`, `docs/wiki/knowledge-graph.md`, `docs/fact-extraction-pipeline.md`

- [ ] **Step 1: Bump workspace version**

In `Cargo.toml`, change `version = "0.46.1"` to `version = "0.47.0"`.

- [ ] **Step 2: Update CHANGELOG.md**

Add a new section at the top:

```markdown
## [0.47.0] — 2026-06-16

### Added

- **Relationship type alias resolution (Issue #133)**: `ensure_relationship_type` now resolves names through the `relationship_type_aliases` table before creating a new canonical type. New canonical types automatically register their normalized name as a self-alias. `get_relationship_type_id` also resolves aliases.

### Changed

- `mimir-knowledge/src/extract.rs::normalize_predicate` is now deprecated. Fact extraction relies on the alias table via `ensure_relationship_type`; the hardcoded synonym map remains only as a fallback.

### Migration

- Added migration `036_seed_relationship_type_aliases.sql` which backfills self-aliases for all existing relationship types and seeds legacy hardcoded synonyms (e.g., `attended` → `studied_at`) into the alias table.

### Tests

- Added `ensure_relationship_type_resolves_alias_to_canonical` and `ensure_relationship_type_creates_new_type_and_self_alias` to `mimir-knowledge/tests/relationship_type_dag_test.rs`.

### Documentation

- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/knowledge-graph.md`, and `docs/fact-extraction-pipeline.md` to describe alias-aware resolution.
```

- [ ] **Step 3: Update docs**

- `docs/knowledge-graph-schema.md`: in the relationship-type registry section, state that `ensure_relationship_type` first queries `relationship_type_aliases`, then creates a new canonical type + self-alias on miss.
- `docs/wiki/knowledge-graph.md`: add a short paragraph that relationship-type aliases are resolved automatically when facts are inserted.
- `docs/fact-extraction-pipeline.md`: update the `normalize_predicate` paragraph to note the alias table is now primary and the hardcoded map is deprecated.
- `docs/wiki/what-works-now.md`: update header version to `0.47.0` and add alias-aware resolution note to the Knowledge Graph section.

- [ ] **Step 4: Run formatting and linting**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml CHANGELOG.md docs/wiki/what-works-now.md docs/knowledge-graph-schema.md docs/wiki/knowledge-graph.md docs/fact-extraction-pipeline.md
git commit -m "docs: bump version and document alias-aware relationship type resolution (#133)"
```

---

## Task 6: Code Review

**Files:**
- Review all files touched in Tasks 1–5.

- [ ] **Step 1: Run code review checklist**

Check dimensions: code quality, performance, security, doc comments, DRY, modern design patterns, guideline compliance, VISION compliance, type consistency, public API surface changes.

- [ ] **Step 2: Action every finding**

No optional findings. Re-run `cargo test --workspace`, `cargo fmt --all`, and `cargo clippy --workspace --all-targets --all-features -- -D warnings` after each fix.

- [ ] **Step 3: Commit review fixes**

```bash
git commit -m "review: action code review findings for #133"
```

---

## Task 7: Publish Branch & Open Draft PR

- [ ] **Step 1: Push branch**

```bash
git push origin feat/issue-133-relationship-type-alias-resolution
```

- [ ] **Step 2: Open draft PR**

Use the GitHub app or `gh pr create --draft`:

```bash
gh pr create --draft \
  --title "feat: relationship type alias resolution in ensure_relationship_type" \
  --body "Closes #133

- Makes ensure_relationship_type the single source of truth for relationship-type alias resolution.
- Adds migration 036 to backfill self-aliases and legacy hardcoded synonyms.
- Deprecates normalize_predicate hardcoded map in extract.rs.
- Updates docs and bumps version to 0.47.0.

See docs/superpowers/plans/2026-06-16-issue-133-relationship-type-alias-resolution.md for the full implementation plan."
```

Expected: Draft PR created linking to #133.

</proposed_plan>
