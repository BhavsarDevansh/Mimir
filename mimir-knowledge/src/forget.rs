//! Cascade forget: soft-delete facts to trash, evaluate inferred children,
//! and hard-delete expired trash rows.

use chrono::{DateTime, Duration, Utc};
use serde_json;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use std::pin::Pin;

use crate::KnowledgeError;
use crate::confidence;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::Fact;
use crate::models::source::Source;
use crate::models::trash::TrashPayload;

// ---------------------------------------------------------------------------
// Bulk forget types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ForgetFilters {
    pub fact_id: Option<i32>,
    pub predicate: Option<String>,
    pub subject: Option<String>,
    pub entity: Option<String>,
    pub source: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub all: bool,
}

impl ForgetFilters {
    pub fn is_full_reset(&self) -> bool {
        self.all
    }
}

#[derive(Debug, Clone, Default)]
pub struct ForgetOptions {
    pub yes: bool,
    pub confirm_sensitive: bool,
    pub confirmation_phrase: Option<String>,
    pub archive: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ForgetResult {
    pub forgotten_count: u64,
    pub backup_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Soft-delete a single fact (existing API preserved).
pub async fn forget_fact(
    pool: &SqlitePool,
    fact_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    forget_fact_inner(pool, fact_id, changed_by, now).await
}

/// Bulk forget dispatch.
pub async fn forget_facts(
    pool: &SqlitePool,
    filters: ForgetFilters,
    opts: ForgetOptions,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    if filters.is_full_reset() {
        return forget_all(pool, opts, changed_by, now).await;
    }

    let ids = query_matching_fact_ids(pool, &filters).await?;
    if ids.is_empty() {
        return Ok(ForgetResult {
            forgotten_count: 0,
            backup_path: None,
        });
    }

    let count = ids.len() as u64;

    if count > 100 && !opts.yes {
        return Err(KnowledgeError::Validation(format!(
            "Refusing to forget {} facts. Use --yes to confirm.",
            count
        )));
    }

    let sensitive = has_sensitive_match(pool, &filters).await?;
    if sensitive && !opts.confirm_sensitive {
        return Err(KnowledgeError::Validation(
            "This includes sensitive facts. Use --confirm-sensitive.".to_string(),
        ));
    }

    let mut all_children: Vec<(i32, bool)> = Vec::new();
    for chunk in ids.chunks(50) {
        let mut tx = pool.begin().await?;
        for fact_id in chunk {
            let children = forget_fact_tx(&mut tx, *fact_id, changed_by, now).await?;
            all_children.extend(children);
        }
        tx.commit().await?;
    }

    let deduped: Vec<(i32, bool)> = {
        let mut seen = HashSet::new();
        all_children
            .into_iter()
            .filter(|(id, _)| seen.insert(*id))
            .collect()
    };

    evaluate_children(pool, deduped, now).await?;

    Ok(ForgetResult {
        forgotten_count: count,
        backup_path: None,
    })
}

/// Hard-delete all facts after creating a backup.
async fn forget_all(
    pool: &SqlitePool,
    opts: ForgetOptions,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    if opts.confirmation_phrase.as_deref() != Some("DELETE EVERYTHING") {
        return Err(KnowledgeError::Validation(
            "Full reset requires typing 'DELETE EVERYTHING'.".to_string(),
        ));
    }

    let backup_path = create_backup(pool).await?;

    if opts.archive {
        let ids: Vec<i32> = sqlx::query_scalar("SELECT id FROM facts")
            .fetch_all(pool)
            .await?;
        let count = ids.len() as u64;
        let mut all_children: Vec<(i32, bool)> = Vec::new();
        for chunk in ids.chunks(50) {
            let mut tx = pool.begin().await?;
            for fact_id in chunk {
                let children = forget_fact_tx(&mut tx, *fact_id, changed_by, now).await?;
                all_children.extend(children);
            }
            tx.commit().await?;
        }
        let deduped: Vec<(i32, bool)> = {
            let mut seen = HashSet::new();
            all_children
                .into_iter()
                .filter(|(id, _)| seen.insert(*id))
                .collect()
        };
        evaluate_children(pool, deduped, now).await?;
        return Ok(ForgetResult {
            forgotten_count: count,
            backup_path: Some(backup_path),
        });
    }

    let count = hard_delete_all_facts(pool).await?;
    Ok(ForgetResult {
        forgotten_count: count,
        backup_path: Some(backup_path),
    })
}

/// Create a timestamped backup of the database.
async fn create_backup(pool: &SqlitePool) -> Result<PathBuf, KnowledgeError> {
    let data_dir = mimir_core::paths::data_dir().map_err(|e| {
        KnowledgeError::Validation(format!("Could not resolve data directory: {}", e))
    })?;
    let backup_dir = data_dir.join("backups");
    tokio::fs::create_dir_all(&backup_dir).await?;

    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let backup_path = backup_dir.join(format!("knowledge.db.bak-{}", timestamp));

    let path_str = backup_path.display().to_string().replace("'", "''");
    let query = format!("VACUUM INTO '{}'", path_str);
    sqlx::query(sqlx::AssertSqlSafe(query))
        .execute(pool)
        .await?;

    Ok(backup_path)
}

/// Hard-delete every fact, entity, preference, queue, and trash row.
async fn hard_delete_all_facts(pool: &SqlitePool) -> Result<u64, KnowledgeError> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM fact_dependencies")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sources").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM fact_audit_log")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM preference_sources")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM preference_contexts")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM preferences")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_dates")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_locations")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_aliases")
        .execute(&mut *tx)
        .await?;
    let delete_result = sqlx::query("DELETE FROM facts").execute(&mut *tx).await?;
    let count = delete_result.rows_affected();
    sqlx::query("DELETE FROM entities")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM trash").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM dedup_queue")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM entity_merge_queue")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(count)
}

// ---------------------------------------------------------------------------
// Core per-fact forget logic
// ---------------------------------------------------------------------------

/// Core forget logic executed inside a transaction.
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
        "SELECT id, fact_id, source_type_id, connector_id, connector_type_id, raw_reference,          extracted_at, extraction_method_id          FROM sources WHERE fact_id = ?",
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

fn forget_fact_inner<'a>(
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

/// Query matching fact IDs for bulk forget.
/// Query matching fact IDs for bulk forget.
async fn query_matching_fact_ids(
    pool: &SqlitePool,
    filters: &ForgetFilters,
) -> Result<Vec<i32>, KnowledgeError> {
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT f.id FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE 1=1",
    );

    if let Some(id) = filters.fact_id {
        builder.push(" AND f.id = ");
        builder.push_bind(id);
    }
    if let Some(ref pred) = filters.predicate {
        builder.push(" AND rt.name = ");
        builder.push_bind(pred);
    }
    if let Some(ref subj) = filters.subject {
        builder.push(" AND s.name = ");
        builder.push_bind(subj);
    }
    if let Some(ref ent) = filters.entity {
        builder.push(" AND (s.name = ");
        builder.push_bind(ent);
        builder.push(" OR o.name = ");
        builder.push_bind(ent);
        builder.push(")");
    }
    if let Some(ref src) = filters.source {
        builder.push(" AND f.id IN (SELECT so.fact_id FROM sources so WHERE so.connector_id = ");
        builder.push_bind(src);
        builder.push(" OR so.source_type_id = (SELECT id FROM source_types WHERE name = ");
        builder.push_bind(src);
        builder.push("))");
    }
    if let Some(from) = filters.from {
        builder.push(" AND f.created_at >= ");
        builder.push_bind(from);
    }
    if let Some(to) = filters.to {
        builder.push(" AND f.created_at <= ");
        builder.push_bind(to);
    }

    let ids: Vec<i32> = builder.build_query_scalar::<i32>().fetch_all(pool).await?;
    Ok(ids)
}

/// Check whether any matching fact is tagged sensitive.
async fn has_sensitive_match(
    pool: &SqlitePool,
    filters: &ForgetFilters,
) -> Result<bool, KnowledgeError> {
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT 1 FROM facts f JOIN entities s ON s.id = f.subject_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id WHERE rt.sensitive = TRUE",
    );

    if let Some(id) = filters.fact_id {
        builder.push(" AND f.id = ");
        builder.push_bind(id);
    }
    if let Some(ref pred) = filters.predicate {
        builder.push(" AND rt.name = ");
        builder.push_bind(pred);
    }
    if let Some(ref subj) = filters.subject {
        builder.push(" AND s.name = ");
        builder.push_bind(subj);
    }
    if let Some(ref ent) = filters.entity {
        builder.push(" AND (s.name = ");
        builder.push_bind(ent);
        builder.push(" OR o.name = ");
        builder.push_bind(ent);
        builder.push(")");
    }
    if let Some(ref src) = filters.source {
        builder.push(" AND f.id IN (SELECT so.fact_id FROM sources so WHERE so.connector_id = ");
        builder.push_bind(src);
        builder.push(" OR so.source_type_id = (SELECT id FROM source_types WHERE name = ");
        builder.push_bind(src);
        builder.push("))");
    }
    if let Some(from) = filters.from {
        builder.push(" AND f.created_at >= ");
        builder.push_bind(from);
    }
    if let Some(to) = filters.to {
        builder.push(" AND f.created_at <= ");
        builder.push_bind(to);
    }
    builder.push(" LIMIT 1");

    let row = builder.build().fetch_optional(pool).await?;
    Ok(row.is_some())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forget_filters_full_reset() {
        let mut f = ForgetFilters::default();
        assert!(!f.is_full_reset());
        f.all = true;
        assert!(f.is_full_reset());
    }
}
