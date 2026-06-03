//! Cascade forget: soft-delete facts to trash, evaluate inferred children,
//! and hard-delete expired trash rows.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json;
use sqlx::SqlitePool;
use std::pin::Pin;

use crate::KnowledgeError;
use crate::confidence;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::Fact;
use crate::models::source::Source;

/// Payload stored in `trash` for a forgotten fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashPayload {
    pub fact: Fact,
    pub sources: Vec<Source>,
}

/// Soft-delete a fact into the trash table, evaluate downstream inferred facts,
/// and write an audit log entry.
///
/// Steps:
/// 1. Serialize fact + sources into `trash.payload`.
/// 2. Insert trash row with 30-day expiry.
/// 3. Delete `fact_dependencies` rows (RESTRICT FK prevents DB cascade).
/// 4. Delete the fact from `facts` (`sources` / `fact_audit_log` cascade).
/// 5. For each former child: recalculate confidence; if zero parents +
///    inferred → recursively forget.
pub async fn forget_fact(
    pool: &SqlitePool,
    fact_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    forget_fact_inner(pool, fact_id, changed_by, now).await
}

fn forget_fact_inner<'a>(
    pool: &'a SqlitePool,
    fact_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), KnowledgeError>> + 'a>> {
    Box::pin(async move {
        let mut tx = pool.begin().await?;

        // Fetch the fact.
        let fact: Option<Fact> = sqlx::query_as::<_, Fact>(
            "SELECT id, subject_id, predicate_id, object_id, object_literal, \
             valid_from, valid_until, confidence, fact_status_id, inferred, \
             inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
             FROM facts WHERE id = ?",
        )
        .bind(fact_id)
        .fetch_optional(&mut *tx)
        .await?;

        let fact = fact.ok_or(KnowledgeError::FactNotFound(fact_id))?;

        // Fetch linked sources.
        let sources: Vec<Source> = sqlx::query_as::<_, Source>(
            "SELECT id, fact_id, source_type_id, connector_id, connector_type_id, raw_reference, \
             extracted_at, extraction_method_id \
             FROM sources WHERE fact_id = ?",
        )
        .bind(fact_id)
        .fetch_all(&mut *tx)
        .await?;

        let payload = TrashPayload {
            fact: fact.clone(),
            sources,
        };
        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| KnowledgeError::Validation(format!("JSON serialization failed: {}", e)))?;

        let expires_at = now + Duration::days(30);

        // Insert trash row.
        sqlx::query(
            "INSERT INTO trash (original_table, original_id, payload, deleted_at, expires_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind("facts")
        .bind(fact_id)
        .bind(payload_json)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;

        // Audit log before deletion (FK removed by migration 018 so row persists after delete).
        let old_json = serde_json::to_string(&fact)
            .map_err(|e| KnowledgeError::Validation(format!("JSON serialization failed: {}", e)))?;
        sqlx::query(
            "INSERT INTO fact_audit_log \
             (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(fact_id)
        .bind(ChangeType::Forgotten as i16)
        .bind(old_json)
        .bind(None::<&str>)
        .bind(now)
        .bind(changed_by as i16)
        .bind(None::<&str>)
        .execute(&mut *tx)
        .await?;

        // Identify children before removing dependency rows.
        let children: Vec<(i32, bool)> = sqlx::query_as(
            "SELECT fd.child_fact_id, f.inferred \
             FROM fact_dependencies fd \
             JOIN facts f ON f.id = fd.child_fact_id \
             WHERE fd.parent_fact_id = ? AND fd.relation_type_id = ?",
        )
        .bind(fact_id)
        .bind(crate::models::enums::RelationType::InferredFrom as i16)
        .fetch_all(&mut *tx)
        .await?;

        // Remove all dependency rows where this fact is parent or child.
        sqlx::query("DELETE FROM fact_dependencies WHERE parent_fact_id = ? OR child_fact_id = ?")
            .bind(fact_id)
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;

        // Hard-delete the fact. sources cascade; fact_audit_log rows persist (migration 018).
        sqlx::query("DELETE FROM facts WHERE id = ?")
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        // Evaluate former children outside the primary transaction so that
        // each recursive forget runs its own transaction.
        for (child_id, child_inferred) in children {
            let remaining_parents: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM fact_dependencies WHERE child_fact_id = ? AND relation_type_id = ?",
            )
            .bind(child_id)
            .bind(crate::models::enums::RelationType::InferredFrom as i16)
            .fetch_one(pool)
            .await?;

            if remaining_parents == 0 && child_inferred {
                forget_fact_inner(pool, child_id, ChangedBy::System, now).await?;
            } else {
                let old_confidence: Option<f32> =
                    sqlx::query_scalar("SELECT confidence FROM facts WHERE id = ?")
                        .bind(child_id)
                        .fetch_optional(pool)
                        .await?;
                let old_confidence = old_confidence.unwrap_or(0.0);

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
                        "INSERT INTO fact_audit_log \
                         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
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
                        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
                         valid_from, valid_until, confidence, fact_status_id, inferred, \
                         inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
                         FROM facts WHERE id = ?",
                    )
                    .bind(child_id)
                    .fetch_optional(&mut *tx)
                    .await?;

                    if let Some(old_child) = old_child {
                        let old_json =
                            serde_json::to_string(&old_child.fact_status_id).map_err(|e| {
                                KnowledgeError::Validation(format!(
                                    "JSON serialization failed: {}",
                                    e
                                ))
                            })?;

                        sqlx::query(
                            "UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?",
                        )
                        .bind(crate::models::fact::FactStatus::Disputed as i16)
                        .bind(now)
                        .bind(child_id)
                        .execute(&mut *tx)
                        .await?;

                        let updated_child: Fact = sqlx::query_as::<_, Fact>(
                            "SELECT id, subject_id, predicate_id, object_id, object_literal, \
                             valid_from, valid_until, confidence, fact_status_id, inferred, \
                             inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
                             FROM facts WHERE id = ?",
                        )
                        .bind(child_id)
                        .fetch_one(&mut *tx)
                        .await?;

                        let new_json = serde_json::to_string(&updated_child.fact_status_id)
                            .map_err(|e| {
                                KnowledgeError::Validation(format!(
                                    "JSON serialization failed: {}",
                                    e
                                ))
                            })?;
                        sqlx::query(
                            "INSERT INTO fact_audit_log \
                             (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                             VALUES (?, ?, ?, ?, ?, ?, ?)",
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
    })
}

/// Hard-delete trash rows that have passed their expiry date.
pub async fn hard_delete_expired_trash(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<u64, KnowledgeError> {
    let result = sqlx::query("DELETE FROM trash WHERE expires_at < ? AND original_table = 'facts'")
        .bind(now)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}
