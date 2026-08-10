//! Fact field updates with per-field audit entries.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::{Fact, FactStatus};
#[allow(clippy::too_many_arguments)]
pub async fn update_fact(
    pool: &SqlitePool,
    fact_id: i32,
    confidence: Option<f32>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    object_literal: Option<String>,
    status: Option<FactStatus>,
    now: DateTime<Utc>,
    changed_by: ChangedBy,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \n         valid_from, valid_until, confidence, fact_status_id, inferred, \n         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \n         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;

    let old = old.ok_or(KnowledgeError::FactNotFound(fact_id))?;

    // Validate temporal ordering if both are changing or one is.
    let new_from = valid_from.or(old.valid_from);
    let new_until = valid_until.or(old.valid_until);
    if let (Some(from), Some(until)) = (new_from, new_until) {
        if until < from {
            return Err(KnowledgeError::Validation(format!(
                "valid_until ({}) must not be before valid_from ({})",
                until, from
            )));
        }
    }

    let mut updates: Vec<(&str, ChangeType, Option<String>, Option<String>)> = Vec::new();

    if let Some(c) = confidence {
        let old_json = serde_json::json!({"confidence": old.confidence}).to_string();
        let new_json = serde_json::json!({"confidence": c}).to_string();
        sqlx::query("UPDATE facts SET confidence = ?, updated_at = ? WHERE id = ?")
            .bind(c)
            .bind(now)
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;
        updates.push((
            "confidence",
            ChangeType::ConfidenceChange,
            Some(old_json),
            Some(new_json),
        ));
    }

    if valid_from.is_some() || valid_until.is_some() {
        let old_json =
            serde_json::json!({"valid_from": old.valid_from, "valid_until": old.valid_until})
                .to_string();
        let new_json =
            serde_json::json!({"valid_from": new_from, "valid_until": new_until}).to_string();
        sqlx::query(
            "UPDATE facts SET valid_from = ?, valid_until = ?, updated_at = ? WHERE id = ?",
        )
        .bind(new_from)
        .bind(new_until)
        .bind(now)
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;
        updates.push((
            "temporal",
            ChangeType::TemporalUpdate,
            Some(old_json),
            Some(new_json),
        ));
    }

    if let Some(ref lit) = object_literal {
        let old_json = serde_json::json!({"object_literal": old.object_literal}).to_string();
        let new_json = serde_json::json!({"object_literal": lit}).to_string();
        sqlx::query("UPDATE facts SET object_literal = ?, updated_at = ? WHERE id = ?")
            .bind(lit)
            .bind(now)
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;
        updates.push((
            "object_literal",
            ChangeType::ContentUpdate,
            Some(old_json),
            Some(new_json),
        ));
    }

    if let Some(s) = status {
        let old_json = serde_json::json!({"fact_status_id": old.fact_status_id}).to_string();
        let new_json = serde_json::json!({"fact_status_id": s as i16}).to_string();
        sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
            .bind(s as i16)
            .bind(now)
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;
        updates.push((
            "status",
            ChangeType::StatusChange,
            Some(old_json),
            Some(new_json),
        ));
    }

    let updated: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \n         valid_from, valid_until, confidence, fact_status_id, inferred, \n         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \n         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut *tx)
    .await?;

    for (_field, change_type, old_value, new_value) in updates {
        sqlx::query(
            "INSERT INTO fact_audit_log \n             (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \n             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(fact_id)
        .bind(change_type as i16)
        .bind(old_value)
        .bind(new_value)
        .bind(now)
        .bind(changed_by as i16)
        .bind(None::<&str>)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(updated)
}
