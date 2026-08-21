//! Temporal and status transitions: valid-until closes, status changes, overlap.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::{Fact, FactStatus};
pub async fn update_valid_until(
    pool: &SqlitePool,
    fact_id: i32,
    new_valid_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    changed_by: ChangedBy,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;

    let old = old.ok_or(KnowledgeError::FactNotFound(fact_id))?;

    if let (Some(from), Some(new_until)) = (old.valid_from, new_valid_until) {
        if new_until < from {
            return Err(KnowledgeError::Validation(format!(
                "valid_until ({}) must not be before valid_from ({})",
                new_until, from
            )));
        }
    }

    let old_json = serde_json::json!({"valid_until": old.valid_until}).to_string();

    sqlx::query("UPDATE facts SET valid_until = ?, updated_at = ? WHERE id = ?")
        .bind(new_valid_until)
        .bind(now)
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;

    let updated: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_json = serde_json::json!({"valid_until": updated.valid_until}).to_string();
    sqlx::query(
        "INSERT INTO fact_audit_log \
         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(ChangeType::TemporalUpdate as i16)
    .bind(old_json)
    .bind(new_json)
    .bind(now)
    .bind(changed_by as i16)
    .bind(None::<&str>)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(updated)
}

/// Update the lifecycle status of a fact, writing a `status_change` audit entry.
pub async fn set_status(
    pool: &SqlitePool,
    fact_id: i32,
    new_status: FactStatus,
    now: DateTime<Utc>,
    changed_by: ChangedBy,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;
    let updated = set_status_tx(&mut tx, fact_id, new_status, now, changed_by).await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn set_status_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fact_id: i32,
    new_status: FactStatus,
    now: DateTime<Utc>,
    changed_by: ChangedBy,
) -> Result<Fact, KnowledgeError> {
    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut **tx)
    .await?;

    let old = old.ok_or(KnowledgeError::FactNotFound(fact_id))?;
    let old_json = serde_json::json!({"fact_status_id": old.fact_status_id}).to_string();

    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
        .bind(new_status as i16)
        .bind(now)
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;

    // A superseded fact is no longer a real event: retire its overlay so it
    // stops advancing and surfacing (issue #413). Centralised here so every
    // supersession path (insert pipeline, inference, user status edits) keeps
    // the overlay lifecycle in sync with the fact status.
    if new_status == FactStatus::Superseded {
        crate::queries::event::retire_overlay_for_fact_in_tx(tx, fact_id, now).await?;
    }

    let updated: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut **tx)
    .await?;

    let new_json = serde_json::json!({"fact_status_id": updated.fact_status_id}).to_string();
    sqlx::query(
        "INSERT INTO fact_audit_log \
         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(ChangeType::StatusChange as i16)
    .bind(old_json)
    .bind(new_json)
    .bind(now)
    .bind(changed_by as i16)
    .bind(None::<&str>)
    .execute(&mut **tx)
    .await?;

    Ok(updated)
}
pub fn ranges_overlap(
    a_from: Option<DateTime<Utc>>,
    a_until: Option<DateTime<Utc>>,
    b_from: Option<DateTime<Utc>>,
    b_until: Option<DateTime<Utc>>,
) -> bool {
    let a_starts_before_b_ends = match (a_from, b_until) {
        (None, _) => true,
        (_, None) => true,
        (Some(af), Some(bu)) => af < bu,
    };
    let b_starts_before_a_ends = match (b_from, a_until) {
        (None, _) => true,
        (_, None) => true,
        (Some(bf), Some(au)) => bf < au,
    };
    a_starts_before_b_ends && b_starts_before_a_ends
}
