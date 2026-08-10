//! Bulk forget machinery: matching, batching, backup, and trash writes.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::KnowledgeError;
use crate::models::audit_log::ChangedBy;

use super::cascade::{evaluate_children, forget_fact_inner, forget_fact_tx};
use super::{ForgetFilters, ForgetOptions, ForgetResult};

const TRASH_BATCH_SIZE: usize = 50;

/// Trash every fact in `ids` in transactional batches, then evaluate the
/// inferred children they leave behind.
///
/// Shared by [`forget_facts`], [`forget_all`], and
/// [`forget_facts_for_connector`] so the batching, transaction boundary, and
/// child-deduplication rules have one definition.
async fn trash_ids_in_batches(
    pool: &SqlitePool,
    ids: &[i32],
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    let mut all_children: Vec<(i32, bool)> = Vec::new();
    for chunk in ids.chunks(TRASH_BATCH_SIZE) {
        let mut tx = pool.begin().await?;
        for fact_id in chunk {
            all_children.extend(forget_fact_tx(&mut tx, *fact_id, changed_by, now).await?);
        }
        tx.commit().await?;
    }

    let mut seen = HashSet::new();
    let deduped: Vec<(i32, bool)> = all_children
        .into_iter()
        .filter(|(id, _)| seen.insert(*id))
        .collect();

    evaluate_children(pool, deduped, now).await
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

    trash_ids_in_batches(pool, &ids, changed_by, now).await?;

    Ok(ForgetResult {
        forgotten_count: count,
        backup_path: None,
    })
}

/// Soft-delete (trash) every fact sourced from a single connector instance
/// (Phase 3 A2 / #203).
///
/// The connector `forget` cascade: selects every fact id whose `sources`
/// row carries `connector_instance_id = instance_id`, then trashes each via
/// `forget_fact_tx` — the same trash machinery as [`forget_facts`] — so the
/// facts are recoverable from trash (30-day expiry) rather than hard-deleted.
/// Unlike the generic [`forget_facts`], no `--yes` / `--confirm-sensitive`
/// gate applies: a connector `forget` is an explicit admin action that
/// removes *all* of the connector's facts, sensitive or not. Inferred child
/// facts are evaluated via `evaluate_children` as usual. A fact sourced
/// from *both* the connector and an independent source (e.g. a chat turn) is
/// trashed wholesale — the connector source is the trigger and the fact is
/// recoverable from trash — so the cascade does not preserve facts that a
/// connector corroborated.
///
/// The caller (the server route) deletes the connector row and its stored
/// secret separately after this returns.
pub async fn forget_facts_for_connector(
    pool: &SqlitePool,
    instance_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<ForgetResult, KnowledgeError> {
    let ids: Vec<i32> = sqlx::query_scalar(
        "SELECT DISTINCT so.fact_id FROM sources so WHERE so.connector_instance_id = ?",
    )
    .bind(instance_id)
    .fetch_all(pool)
    .await?;

    let count = ids.len() as u64;
    if count == 0 {
        return Ok(ForgetResult {
            forgotten_count: 0,
            backup_path: None,
        });
    }

    trash_ids_in_batches(pool, &ids, changed_by, now).await?;

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
        trash_ids_in_batches(pool, &ids, changed_by, now).await?;
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
        builder.push(" AND f.id IN (SELECT so.fact_id FROM sources so WHERE so.connector_instance_id IN (SELECT id FROM connectors WHERE slug = ");
        builder.push_bind(src);
        builder.push(") OR so.source_type_id = (SELECT id FROM source_types WHERE name = ");
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
        builder.push(" AND f.id IN (SELECT so.fact_id FROM sources so WHERE so.connector_instance_id IN (SELECT id FROM connectors WHERE slug = ");
        builder.push_bind(src);
        builder.push(") OR so.source_type_id = (SELECT id FROM source_types WHERE name = ");
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
