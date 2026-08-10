//! Duplicate detection and merging: exact duplicates, overlapping aliases,
//! semantic-dedup queue.

use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity::Entity;
use crate::models::enums::MergeWorkflowStatus;
use crate::queries::entity::crud::get_by_id;

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

pub async fn enqueue_semantic_dedup(
    _pool: &SqlitePool,
    _candidate_pairs: Vec<(Entity, Entity)>,
) -> Result<(), KnowledgeError> {
    // TODO(#50): Build structured prompt, call LlmWorkerPool, parse JSON response,
    // insert into entity_merge_queue with llm_confidence and suggested_action.
    Err(KnowledgeError::NotYetImplemented)
}
