//! Entity CRUD and name/alias search.

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::KnowledgeError;
use crate::models::entity::{Entity, EntityType};

// ---------------------------------------------------------------------------
// Alias search
// ---------------------------------------------------------------------------

/// How an entity was matched during name/alias search.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MatchKind {
    ExactName,
    ExactAlias,
    Fuzzy,
}

/// Result of searching for an entity by name or alias.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AliasSearchResult {
    pub entity: Entity,
    pub match_kind: MatchKind,
    pub score: f32,
}

pub async fn create_entity(
    pool: &SqlitePool,
    name: &str,
    entity_type: EntityType,
    aliases: &[&str],
) -> Result<Entity, KnowledgeError> {
    let aliases_json = if aliases.is_empty() {
        None
    } else {
        Some(serde_json::to_string(aliases).unwrap_or_else(|_| "[]".to_string()))
    };

    let mut tx = pool.begin().await?;

    // Upsert entity under a transaction; the unique index on LOWER(name)
    // guarantees case-insensitive uniqueness.
    let entity: Option<Entity> = sqlx::query_as::<_, Entity>(
        "INSERT INTO entities (name, entity_type_id, aliases) \
         VALUES (?, ?, ?) \
         ON CONFLICT DO NOTHING \
         RETURNING id, name, entity_type_id, aliases, created_at, updated_at",
    )
    .bind(name)
    .bind(entity_type as i16)
    .bind(aliases_json.as_ref())
    .fetch_optional(&mut *tx)
    .await?;

    let entity = match entity {
        Some(e) => e,
        None => {
            // Conflict: another txn inserted the same name; fetch the survivor.
            sqlx::query_as::<_, Entity>(
                "SELECT id, name, entity_type_id, aliases, created_at, updated_at \
                 FROM entities WHERE LOWER(name) = LOWER(?) LIMIT 1",
            )
            .bind(name)
            .fetch_one(&mut *tx)
            .await?
        }
    };

    // Insert all aliases atomically within the same transaction. The SQL shape
    // varies with alias count, so do not cache it as a prepared statement.
    if !aliases.is_empty() {
        alias_insert_builder(entity.id, aliases)
            .build()
            .persistent(false)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(entity)
}

/// Build one idempotent multi-row insert for every caller-supplied alias.
fn alias_insert_builder(entity_id: i32, aliases: &[&str]) -> QueryBuilder<Sqlite> {
    let mut query_builder =
        QueryBuilder::new("INSERT OR IGNORE INTO entity_aliases (entity_id, alias) ");
    query_builder.push_values(aliases.iter().copied(), |mut row, alias| {
        row.push_bind(entity_id).push_bind(alias);
    });
    query_builder
}

/// Retrieve an entity by primary key.
pub async fn get_by_id(pool: &SqlitePool, id: i32) -> Result<Option<Entity>, KnowledgeError> {
    let entity: Option<Entity> = sqlx::query_as::<_, Entity>(
        "SELECT id, name, entity_type_id, aliases, created_at, updated_at FROM entities WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(entity)
}

/// Search for entities by exact name match, then exact alias match, then FTS5 fuzzy.
/// Results are sorted by score descending and capped at 10.
///
/// All three steps are type-agnostic. For the resolution path that must respect
/// an entity's declared type, use [`get_by_name_typed`].
pub async fn get_by_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Vec<AliasSearchResult>, KnowledgeError> {
    search_by_name(pool, name, None).await
}

/// Same three-stage search as [`get_by_name`], restricted to entities of the
/// given type. Cross-type matches are excluded so that, e.g. resolving "Rome"
/// as a [`EntityType::Place`] never merges into a `Person` named "Rome".
///
/// This is the lookup used by entity resolution (`resolve_entity`); the
/// untyped [`get_by_name`] remains the general-purpose search surface.
pub async fn get_by_name_typed(
    pool: &SqlitePool,
    name: &str,
    entity_type: EntityType,
) -> Result<Vec<AliasSearchResult>, KnowledgeError> {
    search_by_name(pool, name, Some(entity_type as i16)).await
}

/// Exact case-insensitive name lookup across all entity types.
///
/// Mirrors the `ON CONFLICT DO NOTHING` upsert in [`create_entity`]: a
/// same-name entity of a different type is reused, not duplicated. The
/// Obsidian import planner (issue #62) uses this as the truthful dry-run
/// counterpart of the typed chain — after a type-filtered miss, an exact
/// same-name entity is the entity `create_entity` would return.
pub async fn get_exact_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<Entity>, KnowledgeError> {
    sqlx::query_as::<_, Entity>(
        "SELECT id, name, entity_type_id, aliases, created_at, updated_at \
         FROM entities WHERE name = ? COLLATE NOCASE LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// List every entity in the graph, ordered by name.
///
/// Backs the Obsidian export snapshot (issue #62) which renders one document
/// per entity.
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Entity>, KnowledgeError> {
    sqlx::query_as::<_, Entity>(
        "SELECT id, name, entity_type_id, aliases, created_at, updated_at \
         FROM entities ORDER BY name, id",
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Core exact-name → exact-alias → FTS5-fuzzy search, optionally restricted to
/// one entity type. When `type_filter` is set, cross-type candidates are
/// dropped after fetch (entity counts are small and personal-scale, so this
/// avoids duplicating the three-step SQL while keeping the public surface DRY).
///
/// Results are sorted by score descending: exact alias (1.1) > exact name (1.0)
/// ≥ fuzzy (≤ 1.0). At equal scores the stable sort preserves insertion order,
/// so an exact name always precedes a fuzzy hit scored 1.0.
async fn search_by_name(
    pool: &SqlitePool,
    name: &str,
    type_filter: Option<i16>,
) -> Result<Vec<AliasSearchResult>, KnowledgeError> {
    let mut results: Vec<AliasSearchResult> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // Type gate shared by all three steps: `None` accepts any type.
    let type_matches = |entity_type_id: i16| type_filter.is_none_or(|t| t == entity_type_id);

    // Step 1: exact name match.
    let exact_name: Vec<Entity> = sqlx::query_as::<_, Entity>(
        "SELECT id, name, entity_type_id, aliases, created_at, updated_at \
         FROM entities WHERE name = ? COLLATE NOCASE",
    )
    .bind(name)
    .fetch_all(pool)
    .await?;

    for e in exact_name {
        if type_matches(e.entity_type_id) && seen_ids.insert(e.id) {
            results.push(AliasSearchResult {
                entity: e,
                match_kind: MatchKind::ExactName,
                score: 1.0,
            });
        }
    }

    // Step 2: exact alias match.
    let alias_matches: Vec<(i32,)> =
        sqlx::query_as("SELECT entity_id FROM entity_aliases WHERE alias = ? COLLATE NOCASE")
            .bind(name)
            .fetch_all(pool)
            .await?;

    for (entity_id,) in alias_matches {
        if !seen_ids.contains(&entity_id) {
            if let Some(e) = get_by_id(pool, entity_id).await? {
                if type_matches(e.entity_type_id) && seen_ids.insert(e.id) {
                    results.push(AliasSearchResult {
                        entity: e,
                        match_kind: MatchKind::ExactAlias,
                        score: 1.1, // Aliases outrank exact-name matches to prefer canonical entities.
                    });
                }
            }
        }
    }

    // Step 3: FTS5 fuzzy search.
    // SQLite FTS5 bm25 rank is negative; more negative = better match.
    let safe_query = mimir_core::fts5::escape_fts5(name);
    let fts_rows: Vec<(i32, f64)> = sqlx::query_as(
        "SELECT rowid, rank FROM entity_fts WHERE entity_fts MATCH ? AND rank <= -0.2 ORDER BY rank LIMIT 10",
    )
    .bind(safe_query)
    .fetch_all(pool)
    .await?;

    for (rowid, rank) in fts_rows {
        if !seen_ids.contains(&rowid) {
            if let Some(e) = get_by_id(pool, rowid).await? {
                if type_matches(e.entity_type_id) && seen_ids.insert(e.id) {
                    // Map bm25 rank to 0..1 score; more negative rank → higher score.
                    let score = ((-rank as f32) * 4.0).clamp(0.0, 1.0);
                    results.push(AliasSearchResult {
                        entity: e,
                        match_kind: MatchKind::Fuzzy,
                        score,
                    });
                }
            }
        }
    }

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    results.truncate(10);
    Ok(results)
}

/// General FTS5-powered entity search.
pub async fn search(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<AliasSearchResult>, KnowledgeError> {
    let safe_query = mimir_core::fts5::escape_fts5(query);
    let fts_rows: Vec<(i32, f64)> = sqlx::query_as(
        "SELECT rowid, rank FROM entity_fts WHERE entity_fts MATCH ? ORDER BY rank LIMIT ?",
    )
    .bind(safe_query)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut results = Vec::new();
    for (rowid, rank) in fts_rows {
        if let Some(e) = get_by_id(pool, rowid).await? {
            // Map bm25 rank to 0..1 score; more negative rank → higher score.
            let score = ((-rank as f32) * 4.0).clamp(0.0, 1.0);
            results.push(AliasSearchResult {
                entity: e,
                match_kind: MatchKind::Fuzzy,
                score,
            });
        }
    }

    Ok(results)
}

/// Update an entity's name and type.
pub async fn update_entity(
    pool: &SqlitePool,
    id: i32,
    name: &str,
    type_id: i16,
) -> Result<Entity, KnowledgeError> {
    let entity = sqlx::query_as::<_, Entity>(
        "UPDATE entities SET name = ?, entity_type_id = ?, updated_at = CURRENT_TIMESTAMP \
         WHERE id = ? \
         RETURNING id, name, entity_type_id, aliases, created_at, updated_at",
    )
    .bind(name)
    .bind(type_id)
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(entity)
}

/// Add an alias to an existing entity.
pub async fn add_alias(
    pool: &SqlitePool,
    entity_id: i32,
    alias: &str,
) -> Result<(), KnowledgeError> {
    sqlx::query("INSERT OR IGNORE INTO entity_aliases (entity_id, alias) VALUES (?, ?)")
        .bind(entity_id)
        .bind(alias)
        .execute(pool)
        .await?;

    // Refresh JSON aliases column on the entity row for FTS5 indexing.
    refresh_entity_aliases_json(pool, entity_id).await?;
    Ok(())
}

/// Remove an alias from an entity.
pub async fn remove_alias(
    pool: &SqlitePool,
    entity_id: i32,
    alias: &str,
) -> Result<(), KnowledgeError> {
    sqlx::query("DELETE FROM entity_aliases WHERE entity_id = ? AND alias = ?")
        .bind(entity_id)
        .bind(alias)
        .execute(pool)
        .await?;

    refresh_entity_aliases_json(pool, entity_id).await?;
    Ok(())
}

async fn refresh_entity_aliases_json(
    pool: &SqlitePool,
    entity_id: i32,
) -> Result<(), KnowledgeError> {
    let aliases: Vec<(String,)> =
        sqlx::query_as("SELECT alias FROM entity_aliases WHERE entity_id = ?")
            .bind(entity_id)
            .fetch_all(pool)
            .await?;

    let json = if aliases.is_empty() {
        None
    } else {
        let vec: Vec<String> = aliases.into_iter().map(|a| a.0).collect();
        Some(serde_json::to_string(&vec).unwrap_or_else(|_| "[]".to_string()))
    };

    sqlx::query("UPDATE entities SET aliases = ? WHERE id = ?")
        .bind(json)
        .bind(entity_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete an entity, rejecting if it is referenced by facts, preferences, or merge-queue entries.
pub async fn delete_entity(pool: &SqlitePool, id: i32) -> Result<(), KnowledgeError> {
    let (fact_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?")
            .bind(id)
            .bind(id)
            .fetch_one(pool)
            .await?;

    let (pref_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM preferences WHERE entity_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;

    let (queue_count,): (i64,) =
        sqlx::query_as(
            "SELECT COUNT(*) FROM entity_merge_queue WHERE primary_entity_id = ? OR duplicate_entity_id = ?"
        )
        .bind(id)
        .bind(id)
        .fetch_one(pool)
        .await?;

    let total = fact_count + pref_count + queue_count;
    if total > 0 {
        return Err(KnowledgeError::EntityHasFacts(total));
    }

    sqlx::query("DELETE FROM entities WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::alias_insert_builder;
    use sqlx::Execute;

    #[test]
    fn alias_insert_uses_single_batched_statement() {
        let aliases = ["Alice", "A.", "Ali"];

        let mut query_builder = alias_insert_builder(7, &aliases);
        let query = query_builder.build();

        assert_eq!(
            query.sql().as_str(),
            "INSERT OR IGNORE INTO entity_aliases (entity_id, alias) \
             VALUES (?, ?), (?, ?), (?, ?)"
        );
    }
}
