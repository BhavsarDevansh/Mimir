//! Entity CRUD, alias resolution, deduplication, dates, locations, and predicate validation.

use serde_json;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity::{Entity, EntityType};
use crate::models::entity_location::EntityLocation;
use crate::models::enums::MergeWorkflowStatus;

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

// ---------------------------------------------------------------------------
// Entity CRUD
// ---------------------------------------------------------------------------

/// Create a new entity, running dedup checks first.
///
/// If an entity with the exact same name (case-insensitive) already exists,
/// returns the existing entity and a `DuplicateEntity` error variant is NOT
/// returned here — the caller gets the existing record directly.
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

    // Insert aliases atomically within the same transaction.
    for alias in aliases {
        sqlx::query("INSERT OR IGNORE INTO entity_aliases (entity_id, alias) VALUES (?, ?)")
            .bind(entity.id)
            .bind(alias)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(entity)
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

// ---------------------------------------------------------------------------
// Predicate validation
// ---------------------------------------------------------------------------

/// Validate whether a predicate is allowed for the given subject/object types.
pub async fn validate_predicate(
    pool: &SqlitePool,
    subject_type: EntityType,
    relationship_type_id: i16,
    object_type: EntityType,
) -> Result<(), KnowledgeError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM relationship_constraints \
         WHERE relationship_type_id = ? AND allowed_subject_type_id = ? AND allowed_object_type_id = ? \
         LIMIT 1",
    )
    .bind(relationship_type_id)
    .bind(subject_type as i16)
    .bind(object_type as i16)
    .fetch_optional(pool)
    .await?;

    if row.is_none() {
        return Err(KnowledgeError::InvalidRelationshipType(
            relationship_type_id,
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entity Locations (stubs)
// ---------------------------------------------------------------------------

/// Insert a location stub (returns the inserted record).
pub async fn insert_location(
    pool: &SqlitePool,
    entity_id: i32,
    location_type_id: i16,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
) -> Result<EntityLocation, KnowledgeError> {
    let record = sqlx::query_as::<_, EntityLocation>(
        "INSERT INTO entity_locations (entity_id, location_type_id, address, latitude, longitude, timezone) \
         VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, created_at",
    )
    .bind(entity_id)
    .bind(location_type_id)
    .bind(address)
    .bind(latitude)
    .bind(longitude)
    .bind(timezone)
    .fetch_one(pool)
    .await?;
    Ok(record)
}

/// Get all locations for an entity.
pub async fn get_locations(
    pool: &SqlitePool,
    entity_id: i32,
) -> Result<Vec<EntityLocation>, KnowledgeError> {
    let rows: Vec<EntityLocation> = sqlx::query_as::<_, EntityLocation>(
        "SELECT id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, created_at \
         FROM entity_locations WHERE entity_id = ? ORDER BY created_at",
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Update a location's fields.
pub async fn update_location(
    pool: &SqlitePool,
    id: i32,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
) -> Result<EntityLocation, KnowledgeError> {
    let record = sqlx::query_as::<_, EntityLocation>(
        "UPDATE entity_locations \
         SET address = COALESCE(?, address), \
             latitude = COALESCE(?, latitude), \
             longitude = COALESCE(?, longitude), \
             timezone = COALESCE(?, timezone) \
         WHERE id = ? \
         RETURNING id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, created_at",
    )
    .bind(address)
    .bind(latitude)
    .bind(longitude)
    .bind(timezone)
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(record)
}

// ---------------------------------------------------------------------------
// Dedup: exact-match auto-merge + overlapping-alias flagging
// ---------------------------------------------------------------------------

/// Find pairs of entities whose names match case-insensitively.
pub async fn find_exact_duplicates(
    pool: &SqlitePool,
) -> Result<Vec<(Entity, Entity)>, KnowledgeError> {
    let rows: Vec<(i32, i32)> = sqlx::query_as(
        "WITH dup_names AS ( \
            SELECT LOWER(name) AS lower_name \
            FROM entities \
            GROUP BY LOWER(name) \
            HAVING COUNT(*) > 1 \
         ) \
         SELECT a.id, b.id \
         FROM entities a \
         JOIN dup_names d ON LOWER(a.name) = d.lower_name \
         JOIN entities b ON LOWER(b.name) = d.lower_name AND a.id < b.id",
    )
    .fetch_all(pool)
    .await?;

    let mut pairs = Vec::new();
    for (id_a, id_b) in rows {
        if let (Some(a), Some(b)) = (get_by_id(pool, id_a).await?, get_by_id(pool, id_b).await?) {
            pairs.push((a, b));
        }
    }
    Ok(pairs)
}

/// Auto-merge two exact-duplicate entities.
///
/// - Facts referencing the merged entity are repointed to the survivor.
/// - Aliases from the merged entity are appended to the survivor.
/// - The merged entity is hard-deleted.
pub async fn auto_merge_pair(
    pool: &SqlitePool,
    survivor_id: i32,
    merged_id: i32,
) -> Result<(), KnowledgeError> {
    if survivor_id == merged_id {
        return Err(KnowledgeError::Validation(
            "survivor and merged IDs must differ".to_string(),
        ));
    }

    // Pick survivor as the one with the most facts (if tied, prefer the lower ID as stable heuristic).
    let (survivor_facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?")
            .bind(survivor_id)
            .bind(survivor_id)
            .fetch_one(pool)
            .await?;

    let (merged_facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?")
            .bind(merged_id)
            .bind(merged_id)
            .fetch_one(pool)
            .await?;

    let (actual_survivor, actual_merged) = if survivor_facts >= merged_facts {
        (survivor_id, merged_id)
    } else {
        (merged_id, survivor_id)
    };

    let mut tx = pool.begin().await?;

    // 1. Repoint facts.
    sqlx::query("UPDATE facts SET subject_id = ? WHERE subject_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    sqlx::query("UPDATE facts SET object_id = ? WHERE object_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 2. Move aliases from merged to survivor (ignore duplicates).
    let aliases: Vec<(String,)> =
        sqlx::query_as("SELECT alias FROM entity_aliases WHERE entity_id = ?")
            .bind(actual_merged)
            .fetch_all(&mut *tx)
            .await?;

    for (alias,) in aliases {
        sqlx::query("INSERT OR IGNORE INTO entity_aliases (entity_id, alias) VALUES (?, ?)")
            .bind(actual_survivor)
            .bind(alias)
            .execute(&mut *tx)
            .await?;
    }

    // 3. Refresh survivor aliases JSON.
    let survivor_aliases: Vec<(String,)> =
        sqlx::query_as("SELECT alias FROM entity_aliases WHERE entity_id = ?")
            .bind(actual_survivor)
            .fetch_all(&mut *tx)
            .await?;

    let json = if survivor_aliases.is_empty() {
        None
    } else {
        let vec: Vec<String> = survivor_aliases.into_iter().map(|a| a.0).collect();
        Some(serde_json::to_string(&vec).unwrap_or_else(|_| "[]".to_string()))
    };

    sqlx::query("UPDATE entities SET aliases = ? WHERE id = ?")
        .bind(json)
        .bind(actual_survivor)
        .execute(&mut *tx)
        .await?;

    // 4. Migrate event overlays to survivor (denormalized entity_id; the
    //    underlying facts were already repointed in step 1).
    sqlx::query("UPDATE events SET entity_id = ? WHERE entity_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 5. Migrate entity_locations to survivor.
    sqlx::query("UPDATE entity_locations SET entity_id = ? WHERE entity_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 6. Remove preferences for merged entity to avoid FK violation.
    sqlx::query("DELETE FROM preferences WHERE entity_id = ?")
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 7. Remove merge-queue entries referencing merged entity.
    sqlx::query(
        "DELETE FROM entity_merge_queue WHERE primary_entity_id = ? OR duplicate_entity_id = ?",
    )
    .bind(actual_merged)
    .bind(actual_merged)
    .execute(&mut *tx)
    .await?;

    // 8. Delete merged entity (cascades entity_aliases thanks to ON DELETE CASCADE).
    sqlx::query("DELETE FROM entities WHERE id = ?")
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

/// Find pairs of entities that share at least one alias string.
pub async fn find_overlapping_aliases(
    pool: &SqlitePool,
) -> Result<Vec<(Entity, Entity, String)>, KnowledgeError> {
    let rows: Vec<(i32, i32, String)> = sqlx::query_as(
        "SELECT a.entity_id, b.entity_id, a.alias \
         FROM entity_aliases a \
         JOIN entity_aliases b ON LOWER(a.alias) = LOWER(b.alias) AND a.entity_id < b.entity_id",
    )
    .fetch_all(pool)
    .await?;

    let mut pairs = Vec::new();
    for (id_a, id_b, alias) in rows {
        if let (Some(a), Some(b)) = (get_by_id(pool, id_a).await?, get_by_id(pool, id_b).await?) {
            pairs.push((a, b, alias));
        }
    }
    Ok(pairs)
}

/// Flag overlapping aliases in the entity_merge_queue for human review.
pub async fn flag_overlapping_aliases(pool: &SqlitePool) -> Result<(), KnowledgeError> {
    let overlaps = find_overlapping_aliases(pool).await?;
    for (a, b, _alias) in overlaps {
        sqlx::query(
            "INSERT OR IGNORE INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) \
             VALUES (?, ?, ?)",
        )
        .bind(a.id)
        .bind(b.id)
        .bind(MergeWorkflowStatus::Pending as i16)
        .execute(pool)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LLM semantic dedup stub
// ---------------------------------------------------------------------------

/// Stub: enqueue a semantic dedup request via the LLM worker pool.
///
/// In #49 this is a stub — it compiles and has the correct signature but
/// returns `Err(KnowledgeError::NotYetImplemented)`.
pub async fn enqueue_semantic_dedup(
    _pool: &SqlitePool,
    _candidate_pairs: Vec<(Entity, Entity)>,
) -> Result<(), KnowledgeError> {
    // TODO(#50): Build structured prompt, call LlmWorkerPool, parse JSON response,
    // insert into entity_merge_queue with llm_confidence and suggested_action.
    Err(KnowledgeError::NotYetImplemented)
}

// ---------------------------------------------------------------------------
// Batch name resolution
// ---------------------------------------------------------------------------

/// Resolve names for a set of entity IDs in a single query.
pub async fn get_entity_names(
    pool: &SqlitePool,
    ids: &[u32],
) -> Result<std::collections::HashMap<u32, String>, KnowledgeError> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT id, name FROM entities WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut query = sqlx::query_as::<_, (i32, String)>(sqlx::AssertSqlSafe(&*sql));
    for &id in ids {
        query = query.bind(id as i32);
    }
    let rows = query.fetch_all(pool).await?;
    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for (id, name) in rows {
        map.insert(id as u32, name);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {}
