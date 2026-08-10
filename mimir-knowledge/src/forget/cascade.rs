//! Recursive child evaluation after a fact is forgotten.

use std::pin::Pin;

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::confidence;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::Fact;
use crate::models::source::Source;
use crate::models::trash::TrashPayload;

pub(crate) async fn forget_fact_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fact_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<Vec<(i32, bool)>, KnowledgeError> {
    let fact: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal,          valid_from, valid_until, confidence, fact_status_id, inferred,          inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at          FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut **tx)
    .await?;

    let fact = fact.ok_or(KnowledgeError::FactNotFound(fact_id))?;

    let sources: Vec<Source> = sqlx::query_as::<_, Source>(
        "SELECT id, fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference,          extracted_at, extraction_method_id          FROM sources WHERE fact_id = ?",
    )
    .bind(fact_id)
    .fetch_all(&mut **tx)
    .await?;

    let dependencies: Vec<(i32, i16)> = sqlx::query_as(
        "SELECT parent_fact_id, relation_type_id FROM fact_dependencies WHERE child_fact_id = ?",
    )
    .bind(fact_id)
    .fetch_all(&mut **tx)
    .await?;

    let payload = TrashPayload {
        fact: fact.clone(),
        sources,
        dependencies,
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| KnowledgeError::Validation(format!("JSON serialization failed: {}", e)))?;

    let expires_at = now + Duration::days(30);

    sqlx::query(
        "INSERT INTO trash (original_table, original_id, payload, deleted_at, expires_at)          VALUES (?, ?, ?, ?, ?)",
    )
    .bind("facts")
    .bind(fact_id)
    .bind(payload_json)
    .bind(now)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;

    let old_json = serde_json::to_string(&fact)
        .map_err(|e| KnowledgeError::Validation(format!("JSON serialization failed: {}", e)))?;
    sqlx::query(
        "INSERT INTO fact_audit_log          (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason)          VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(ChangeType::Forgotten as i16)
    .bind(old_json)
    .bind(None::<&str>)
    .bind(now)
    .bind(changed_by as i16)
    .bind(None::<&str>)
    .execute(&mut **tx)
    .await?;

    let children: Vec<(i32, bool)> = sqlx::query_as(
        "SELECT fd.child_fact_id, f.inferred          FROM fact_dependencies fd          JOIN facts f ON f.id = fd.child_fact_id          WHERE fd.parent_fact_id = ? AND fd.relation_type_id = ?",
    )
    .bind(fact_id)
    .bind(crate::models::enums::RelationType::InferredFrom as i16)
    .fetch_all(&mut **tx)
    .await?;

    sqlx::query("DELETE FROM fact_dependencies WHERE parent_fact_id = ? OR child_fact_id = ?")
        .bind(fact_id)
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query("DELETE FROM facts WHERE id = ?")
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;

    Ok(children)
}

/// Evaluate orphaned children after their parent(s) have been forgotten.
pub(crate) async fn evaluate_children(
    pool: &SqlitePool,
    children: Vec<(i32, bool)>,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    for (child_id, child_inferred) in children {
        let remaining_parents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fact_dependencies WHERE child_fact_id = ? AND relation_type_id = ?",
        )
        .bind(child_id)
        .bind(crate::models::enums::RelationType::InferredFrom as i16)
        .fetch_one(pool)
        .await?;

        if remaining_parents == 0 && child_inferred {
            if let Err(e) = forget_fact_inner(pool, child_id, ChangedBy::System, now).await {
                if !matches!(e, KnowledgeError::FactNotFound(_)) {
                    return Err(e);
                }
            }
        } else {
            let old_confidence: Option<f32> =
                sqlx::query_scalar("SELECT confidence FROM facts WHERE id = ?")
                    .bind(child_id)
                    .fetch_optional(pool)
                    .await?;

            let old_confidence = match old_confidence {
                Some(conf) => conf,
                None => continue,
            };

            let new_confidence = confidence::recalculate(pool, child_id).await?;

            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE facts SET confidence = ?, updated_at = ? WHERE id = ?")
                .bind(new_confidence)
                .bind(now)
                .bind(child_id)
                .execute(&mut *tx)
                .await?;

            if (new_confidence - old_confidence).abs() > 0.001 {
                let old_json = serde_json::json!({"confidence": old_confidence}).to_string();
                let new_json = serde_json::json!({"confidence": new_confidence}).to_string();
                sqlx::query(
                    "INSERT INTO fact_audit_log                      (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason)                      VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(child_id)
                .bind(ChangeType::ConfidenceChange as i16)
                .bind(old_json)
                .bind(new_json)
                .bind(now)
                .bind(ChangedBy::System as i16)
                .bind(None::<&str>)
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;

            if new_confidence < 0.20 {
                let mut tx = pool.begin().await?;

                let old_child: Option<Fact> = sqlx::query_as::<_, Fact>(
                    "SELECT id, subject_id, relationship_type_id, object_id, object_literal,                      valid_from, valid_until, confidence, fact_status_id, inferred,                      inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at                      FROM facts WHERE id = ?",
                )
                .bind(child_id)
                .fetch_optional(&mut *tx)
                .await?;

                if let Some(old_child) = old_child {
                    let old_json =
                        serde_json::to_string(&old_child.fact_status_id).map_err(|e| {
                            KnowledgeError::Validation(format!("JSON serialization failed: {}", e))
                        })?;

                    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
                        .bind(crate::models::fact::FactStatus::Disputed as i16)
                        .bind(now)
                        .bind(child_id)
                        .execute(&mut *tx)
                        .await?;

                    let updated_child: Fact = sqlx::query_as::<_, Fact>(
                        "SELECT id, subject_id, relationship_type_id, object_id, object_literal,                          valid_from, valid_until, confidence, fact_status_id, inferred,                          inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at                          FROM facts WHERE id = ?",
                    )
                    .bind(child_id)
                    .fetch_one(&mut *tx)
                    .await?;

                    let new_json =
                        serde_json::to_string(&updated_child.fact_status_id).map_err(|e| {
                            KnowledgeError::Validation(format!("JSON serialization failed: {}", e))
                        })?;
                    sqlx::query(
                        "INSERT INTO fact_audit_log                      (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason)                      VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(child_id)
                    .bind(ChangeType::StatusChange as i16)
                    .bind(old_json)
                    .bind(new_json)
                    .bind(now)
                    .bind(ChangedBy::System as i16)
                    .bind(None::<&str>)
                    .execute(&mut *tx)
                    .await?;
                }

                tx.commit().await?;
            }
        }
    }

    Ok(())
}

pub(super) fn forget_fact_inner<'a>(
    pool: &'a SqlitePool,
    fact_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), KnowledgeError>> + Send + 'a>> {
    Box::pin(async move {
        let mut tx = pool.begin().await?;
        let children = forget_fact_tx(&mut tx, fact_id, changed_by, now).await?;
        tx.commit().await?;
        evaluate_children(pool, children, now).await
    })
}

// ---------------------------------------------------------------------------
// Filter helpers
// ---------------------------------------------------------------------------
