//! Entity merge-queue review surface: list pending suggestions and resolve
//! them (apply via the existing entity-merge logic, or keep separate).

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::KnowledgeError;
use crate::models::enums::{MergeResolution, MergeWorkflowStatus};

use super::dedup::auto_merge_pair;

/// A pending entity-merge suggestion awaiting human review.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityMergeQueueItem {
    pub id: i64,
    pub primary_entity_id: i32,
    pub primary_name: String,
    pub primary_type: String,
    pub duplicate_entity_id: i32,
    pub duplicate_name: String,
    pub duplicate_type: String,
    /// LLM recommendation (`merge` or `keep_separate`); `None` for rows
    /// flagged deterministically (alias overlap) before LLM evaluation.
    pub suggested_action: Option<String>,
    /// LLM confidence in the suggestion; `None` before LLM evaluation.
    pub llm_confidence: Option<f32>,
    pub queued_at: DateTime<Utc>,
}

/// List pending entity-merge suggestions, newest first.
pub async fn list_pending_merges(
    pool: &SqlitePool,
) -> Result<Vec<EntityMergeQueueItem>, KnowledgeError> {
    let rows = sqlx::query(
        "SELECT q.id, \
                q.primary_entity_id, ep.name AS primary_name, etp.name AS primary_type, \
                q.duplicate_entity_id, ed.name AS duplicate_name, etd.name AS duplicate_type, \
                q.suggested_action, q.llm_confidence, q.queued_at \
         FROM entity_merge_queue q \
         JOIN entities ep ON ep.id = q.primary_entity_id \
         JOIN entities ed ON ed.id = q.duplicate_entity_id \
         JOIN entity_types etp ON etp.id = ep.entity_type_id \
         JOIN entity_types etd ON etd.id = ed.entity_type_id \
         WHERE q.status_id = ? \
         ORDER BY q.queued_at DESC, q.id DESC",
    )
    .bind(MergeWorkflowStatus::Pending as i16)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(EntityMergeQueueItem {
            id: row.try_get("id")?,
            primary_entity_id: row.try_get("primary_entity_id")?,
            primary_name: row.try_get("primary_name")?,
            primary_type: row.try_get("primary_type")?,
            duplicate_entity_id: row.try_get("duplicate_entity_id")?,
            duplicate_name: row.try_get("duplicate_name")?,
            duplicate_type: row.try_get("duplicate_type")?,
            suggested_action: row.try_get("suggested_action")?,
            llm_confidence: row.try_get("llm_confidence")?,
            queued_at: row.try_get("queued_at")?,
        });
    }
    Ok(items)
}

/// Apply a pending merge suggestion using the existing entity-merge logic
/// ([`auto_merge_pair`]): repoint facts, move aliases/overlays/locations,
/// and delete the merged entity.
///
/// The queue row is marked `Processing` first so a concurrent apply cannot
/// double-run; on success [`auto_merge_pair`] removes queue rows referencing
/// the merged entity, so the applied entry no longer appears in the review
/// list. On failure the row is restored to `Pending`.
///
/// Returns the actual `(survivor_id, merged_id)` — [`auto_merge_pair`]
/// picks the survivor by fact count, so the ids may swap relative to the
/// queue row.
pub async fn apply_merge(pool: &SqlitePool, queue_id: i64) -> Result<(i32, i32), KnowledgeError> {
    let row = sqlx::query(
        "SELECT primary_entity_id, duplicate_entity_id FROM entity_merge_queue WHERE id = ?",
    )
    .bind(queue_id)
    .fetch_optional(pool)
    .await?;
    let (primary, duplicate) = match row {
        Some(r) => (
            r.try_get::<i32, _>("primary_entity_id")?,
            r.try_get::<i32, _>("duplicate_entity_id")?,
        ),
        None => {
            return Err(KnowledgeError::Validation(format!(
                "merge queue entry {queue_id} not found"
            )));
        }
    };

    let marked =
        sqlx::query("UPDATE entity_merge_queue SET status_id = ? WHERE id = ? AND status_id = ?")
            .bind(MergeWorkflowStatus::Processing as i16)
            .bind(queue_id)
            .bind(MergeWorkflowStatus::Pending as i16)
            .execute(pool)
            .await?;
    if marked.rows_affected() == 0 {
        return Err(KnowledgeError::Validation(format!(
            "merge queue entry {queue_id} is not pending"
        )));
    }

    if let Err(e) = auto_merge_pair(pool, primary, duplicate).await {
        sqlx::query("UPDATE entity_merge_queue SET status_id = ? WHERE id = ?")
            .bind(MergeWorkflowStatus::Pending as i16)
            .bind(queue_id)
            .execute(pool)
            .await?;
        return Err(e);
    }

    // auto_merge_pair removed the queue row; determine which entity survived.
    let (survivor, merged) = if super::crud::get_by_id(pool, primary).await?.is_some() {
        (primary, duplicate)
    } else {
        (duplicate, primary)
    };
    Ok((survivor, merged))
}

/// Mark a pending merge suggestion as kept separate without merging.
pub async fn keep_merge(pool: &SqlitePool, queue_id: i64) -> Result<(), KnowledgeError> {
    let updated = sqlx::query(
        "UPDATE entity_merge_queue \
         SET status_id = ?, resolution_id = ?, processed_at = ? \
         WHERE id = ? AND status_id = ?",
    )
    .bind(MergeWorkflowStatus::Complete as i16)
    .bind(MergeResolution::KeptSeparate as i16)
    .bind(Utc::now())
    .bind(queue_id)
    .bind(MergeWorkflowStatus::Pending as i16)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(KnowledgeError::Validation(format!(
            "merge queue entry {queue_id} is not pending"
        )));
    }
    Ok(())
}
