//! Trash bin queries: list, restore, empty, expired cleanup.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::confidence;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::{Fact, FactStatus};
use crate::models::trash::{TrashEntry, TrashListItem, TrashPayload};

/// List unrestored trash rows.
pub async fn list_trash(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TrashListItem>, KnowledgeError> {
    let rows: Vec<TrashEntry> = sqlx::query_as::<_, TrashEntry>(
        "SELECT id, original_table, original_id, payload, deleted_at, expires_at, restored_at, restorer          FROM trash          WHERE restored_at IS NULL AND original_table = 'facts'          ORDER BY deleted_at DESC          LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let payload: TrashPayload = serde_json::from_str(&row.payload)
            .map_err(|e| KnowledgeError::Validation(format!("Invalid trash payload: {}", e)))?;
        let fact = &payload.fact;

        let subject_name: Option<String> =
            sqlx::query_scalar("SELECT name FROM entities WHERE id = ?")
                .bind(fact.subject_id)
                .fetch_optional(pool)
                .await?;

        let object_name: Option<String> = if let Some(oid) = fact.object_id {
            sqlx::query_scalar("SELECT name FROM entities WHERE id = ?")
                .bind(oid)
                .fetch_optional(pool)
                .await?
        } else {
            None
        };

        let relationship_type_name: Option<String> =
            sqlx::query_scalar("SELECT name FROM relationship_types WHERE id = ?")
                .bind(fact.relationship_type_id)
                .fetch_optional(pool)
                .await?;

        items.push(TrashListItem {
            trash_id: row.id,
            fact_id: row.original_id,
            subject_name,
            relationship_type_name,
            object_name,
            object_literal: fact.object_literal.clone(),
            deleted_at: row.deleted_at,
            expires_at: row.expires_at,
        });
    }

    Ok(items)
}

/// Restore a single fact from trash.
pub async fn restore_fact(
    pool: &SqlitePool,
    trash_id: i32,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let row: Option<TrashEntry> = sqlx::query_as::<_, TrashEntry>(
        "SELECT id, original_table, original_id, payload, deleted_at, expires_at, restored_at, restorer          FROM trash WHERE id = ? AND restored_at IS NULL",
    )
    .bind(trash_id)
    .fetch_optional(pool)
    .await?;

    let row = row.ok_or_else(|| {
        KnowledgeError::Validation("Trash entry not found or already restored.".to_string())
    })?;
    let payload: TrashPayload = serde_json::from_str(&row.payload)
        .map_err(|e| KnowledgeError::Validation(format!("Invalid trash payload: {}", e)))?;

    // Atomically claim the trash row before restoring.
    let restorer_str = format!("{:?}", changed_by);
    let claim = sqlx::query(
        "UPDATE trash SET restored_at = ?, restorer = ? WHERE id = ? AND restored_at IS NULL",
    )
    .bind(now)
    .bind(&restorer_str)
    .bind(trash_id)
    .execute(pool)
    .await?;
    if claim.rows_affected() == 0 {
        return Err(KnowledgeError::Validation(
            "Trash entry not found or already restored.".to_string(),
        ));
    }

    let fact = restore_payload(pool, payload, changed_by, now).await?;
    Ok(fact)
}

/// Restore all facts from trash (two-pass: facts first, dependencies second).
pub async fn restore_all(
    pool: &SqlitePool,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let rows: Vec<TrashEntry> = sqlx::query_as::<_, TrashEntry>(
        "SELECT id, original_table, original_id, payload, deleted_at, expires_at, restored_at, restorer          FROM trash WHERE restored_at IS NULL AND original_table = 'facts'",
    )
    .fetch_all(pool)
    .await?;

    let mut id_map = std::collections::HashMap::<i32, i32>::new();
    let mut restored_facts = Vec::with_capacity(rows.len());
    let mut all_deps: Vec<(i32, i32, i16)> = Vec::new();

    // Pass 1: restore every fact, building old_id -> new_id map.
    for row in &rows {
        let payload: TrashPayload = serde_json::from_str(&row.payload)
            .map_err(|e| KnowledgeError::Validation(format!("Invalid trash payload: {}", e)))?;
        let old_id = payload.fact.id;

        let new_fact = restore_payload_no_deps(pool, payload, changed_by, now).await?;
        id_map.insert(old_id, new_fact.id);
        restored_facts.push(new_fact);

        let row_deps: Vec<(i32, i16)> = serde_json::from_str(&row.payload)
            .map(|p: TrashPayload| p.dependencies)
            .unwrap_or_default();
        for (parent_old, rel_type) in row_deps {
            all_deps.push((old_id, parent_old, rel_type));
        }
    }

    // Pass 2: rebuild dependencies using the map.
    let mut tx = pool.begin().await?;
    for (child_old, parent_old, rel_type) in &all_deps {
        if let (Some(&child_new), Some(&parent_new)) =
            (id_map.get(child_old), id_map.get(parent_old))
        {
            sqlx::query(
                "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) VALUES (?, ?, ?)",
            )
            .bind(parent_new)
            .bind(child_new)
            .bind(rel_type)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;

    // Recalculate confidence for all restored facts and update in-memory values.
    for fact in &mut restored_facts {
        let new_conf = confidence::recalculate(pool, fact.id).await?;
        sqlx::query("UPDATE facts SET confidence = ? WHERE id = ?")
            .bind(new_conf)
            .bind(fact.id)
            .execute(pool)
            .await?;
        fact.confidence = new_conf;
    }

    // Finalization: mark trash rows restored only after all phases succeed.
    let restorer_str = format!("{:?}", changed_by);
    for row in &rows {
        sqlx::query(
            "UPDATE trash SET restored_at = ?, restorer = ? WHERE id = ? AND restored_at IS NULL",
        )
        .bind(now)
        .bind(&restorer_str)
        .bind(row.id)
        .execute(pool)
        .await?;
    }

    Ok(restored_facts)
}

/// Empty the trash (hard-delete all rows).
pub async fn empty_trash(pool: &SqlitePool) -> Result<u64, KnowledgeError> {
    let result = sqlx::query("DELETE FROM trash WHERE original_table = 'facts'")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async fn restore_payload(
    pool: &SqlitePool,
    payload: TrashPayload,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let mut fact = restore_payload_no_deps(pool, payload.clone(), changed_by, now).await?;

    // Rebuild dependencies where parent still exists.
    let mut tx = pool.begin().await?;
    for (parent_old_id, rel_type) in &payload.dependencies {
        let parent_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts WHERE id = ?")
            .bind(parent_old_id)
            .fetch_one(&mut *tx)
            .await?;
        if parent_exists > 0 {
            sqlx::query(
                "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) VALUES (?, ?, ?)",
            )
            .bind(parent_old_id)
            .bind(fact.id)
            .bind(rel_type)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;

    // Recalculate confidence after dependency rebuild.
    let new_conf = confidence::recalculate(pool, fact.id).await?;
    sqlx::query("UPDATE facts SET confidence = ? WHERE id = ?")
        .bind(new_conf)
        .bind(fact.id)
        .execute(pool)
        .await?;

    fact.confidence = new_conf;
    Ok(fact)
}

async fn restore_payload_no_deps(
    pool: &SqlitePool,
    payload: TrashPayload,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let fact = payload.fact;

    // Check temporal overlap.
    let overlaps: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal,          valid_from, valid_until, confidence, fact_status_id, inferred,          inference_depth, stale_confidence, pending_confirmation, created_at, updated_at          FROM facts          WHERE subject_id = ? AND relationship_type_id = ?",
    )
    .bind(fact.subject_id)
    .bind(fact.relationship_type_id)
    .fetch_all(pool)
    .await?;

    let has_overlap = overlaps.iter().any(|ef| {
        ranges_overlap(
            ef.valid_from,
            ef.valid_until,
            fact.valid_from,
            fact.valid_until,
        )
    });

    let status = if has_overlap {
        FactStatus::Disputed
    } else {
        match fact.status() {
            Some(FactStatus::Forgotten) | Some(FactStatus::Superseded) => FactStatus::Active,
            Some(other) => other,
            None => FactStatus::Active,
        }
    };

    let mut tx = pool.begin().await?;

    let new_fact_id: i64 = sqlx::query_scalar(
        "INSERT INTO facts          (subject_id, relationship_type_id, object_id, object_literal, valid_from, valid_until,           confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation, created_at, updated_at)          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)          RETURNING id",
    )
    .bind(fact.subject_id)
    .bind(fact.relationship_type_id)
    .bind(fact.object_id)
    .bind(&fact.object_literal)
    .bind(fact.valid_from)
    .bind(fact.valid_until)
    .bind(fact.confidence)
    .bind(status as i16)
    .bind(fact.inferred)
    .bind(fact.inference_depth)
    .bind(fact.stale_confidence)
    .bind(fact.pending_confirmation)
    .bind(fact.created_at)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    let new_fact_id = new_fact_id as i32;

    for source in &payload.sources {
        sqlx::query(
            "INSERT INTO sources              (fact_id, source_type_id, connector_id, connector_type_id, raw_reference, extracted_at, extraction_method_id)              VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(new_fact_id)
        .bind(source.source_type_id)
        .bind(&source.connector_id)
        .bind(source.connector_type_id)
        .bind(&source.raw_reference)
        .bind(source.extracted_at)
        .bind(source.extraction_method_id)
        .execute(&mut *tx)
        .await?;
    }

    // Audit log for restoration.
    let new_value = serde_json::json!({"restored_fact_id": new_fact_id}).to_string();
    sqlx::query(
        "INSERT INTO fact_audit_log          (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason)          VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(new_fact_id)
    .bind(ChangeType::Restored as i16)
    .bind(None::<&str>)
    .bind(new_value)
    .bind(now)
    .bind(changed_by as i16)
    .bind(None::<&str>)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let restored: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal,          valid_from, valid_until, confidence, fact_status_id, inferred,          inference_depth, stale_confidence, pending_confirmation, created_at, updated_at          FROM facts WHERE id = ?",
    )
    .bind(new_fact_id)
    .fetch_one(pool)
    .await?;

    Ok(restored)
}

fn ranges_overlap(
    a_from: Option<DateTime<Utc>>,
    a_until: Option<DateTime<Utc>>,
    b_from: Option<DateTime<Utc>>,
    b_until: Option<DateTime<Utc>>,
) -> bool {
    let a_end = a_until.unwrap_or(DateTime::<Utc>::MAX_UTC);
    let b_end = b_until.unwrap_or(DateTime::<Utc>::MAX_UTC);
    let a_start = a_from.unwrap_or(DateTime::<Utc>::MIN_UTC);
    let b_start = b_from.unwrap_or(DateTime::<Utc>::MIN_UTC);
    a_start <= b_end && b_start <= a_end
}
