//! Preference write paths: transactional insert/update, conflict resolution,
//! and audit-log writes.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::preference::{
    NewPreference, Preference, PreferenceContext, UpsertAction, UpsertPreferenceInput,
};

async fn insert_preference_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &UpsertPreferenceInput,
    now: DateTime<Utc>,
) -> Result<Preference, KnowledgeError> {
    // 1. Uniqueness check: same (entity_id, key) + identical context set.
    let candidates: Vec<Preference> = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences \
         WHERE (entity_id IS ?) AND key = ?",
    )
    .bind(input.preference.entity_id)
    .bind(&input.preference.key)
    .fetch_all(&mut **tx)
    .await?;

    let input_len = input.contexts.len();

    for candidate in candidates {
        let ctx_rows: Vec<PreferenceContext> = sqlx::query_as::<_, PreferenceContext>(
            "SELECT id, preference_id, context_key, context_value \
             FROM preference_contexts \
             WHERE preference_id = ?",
        )
        .bind(candidate.id)
        .fetch_all(&mut **tx)
        .await?;

        if ctx_rows.len() == input_len {
            let candidate_ctx_set: HashSet<(&str, &str)> = ctx_rows
                .iter()
                .map(|c| (c.context_key.as_str(), c.context_value.as_str()))
                .collect();

            if input
                .contexts
                .iter()
                .all(|(k, v)| candidate_ctx_set.contains(&(k.as_str(), v.as_str())))
            {
                return Err(KnowledgeError::DuplicatePreference);
            }
        }
    }

    // 2. Insert preference row.
    let pref_id: i32 = sqlx::query_scalar(
        "INSERT INTO preferences \
         (entity_id, category_id, key, value, confidence, overridden_by_user, \
          source_fact_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(input.preference.entity_id)
    .bind(input.preference.category as i16)
    .bind(&input.preference.key)
    .bind(&input.preference.value)
    .bind(input.preference.confidence)
    .bind(input.preference.overridden_by_user)
    .bind(input.preference.source_fact_id)
    .bind(now)
    .bind(now)
    .fetch_one(&mut **tx)
    .await?;

    // 3. Insert context rows.
    for (ctx_key, ctx_value) in &input.contexts {
        sqlx::query(
            "INSERT INTO preference_contexts (preference_id, context_key, context_value) \
             VALUES (?, ?, ?)",
        )
        .bind(pref_id)
        .bind(ctx_key)
        .bind(ctx_value)
        .execute(&mut **tx)
        .await?;
    }

    // 4. Insert sources.
    for (source_type, source_id) in &input.sources {
        sqlx::query(
            "INSERT INTO preference_sources (preference_id, source_type_id, source_id, extracted_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(pref_id)
        .bind(*source_type as i16)
        .bind(source_id)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }

    // 5. Audit log: Created.
    sqlx::query(
        "INSERT INTO preference_audit_log \
         (preference_id, change_type_id, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(pref_id)
    .bind(ChangeType::Created as i16)
    .bind(&input.preference.value)
    .bind(now)
    .bind(input.changed_by as i16)
    .bind(None::<&str>)
    .execute(&mut **tx)
    .await?;

    // 6. Return the inserted preference.
    let pref = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences WHERE id = ?",
    )
    .bind(pref_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(pref)
}

// ---------------------------------------------------------------------------
// Update in-place (preserves preference_id for audit trail)
// ---------------------------------------------------------------------------

async fn update_preference_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    existing_id: i32,
    input: &UpsertPreferenceInput,
    now: DateTime<Utc>,
) -> Result<Preference, KnowledgeError> {
    // 1. Check for duplicate (excluding the row we are updating).
    let candidates: Vec<Preference> = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences \
         WHERE (entity_id IS ?) AND key = ? AND id != ?",
    )
    .bind(input.preference.entity_id)
    .bind(&input.preference.key)
    .bind(existing_id)
    .fetch_all(&mut **tx)
    .await?;

    let input_len = input.contexts.len();

    for candidate in candidates {
        let ctx_rows: Vec<PreferenceContext> = sqlx::query_as::<_, PreferenceContext>(
            "SELECT id, preference_id, context_key, context_value \
             FROM preference_contexts \
             WHERE preference_id = ?",
        )
        .bind(candidate.id)
        .fetch_all(&mut **tx)
        .await?;

        if ctx_rows.len() == input_len {
            let candidate_ctx_set: HashSet<(&str, &str)> = ctx_rows
                .iter()
                .map(|c| (c.context_key.as_str(), c.context_value.as_str()))
                .collect();

            if input
                .contexts
                .iter()
                .all(|(k, v)| candidate_ctx_set.contains(&(k.as_str(), v.as_str())))
            {
                return Err(KnowledgeError::DuplicatePreference);
            }
        }
    }

    // 2. Update preference row.
    sqlx::query(
        "UPDATE preferences SET \
         entity_id = ?, category_id = ?, key = ?, value = ?, \
         confidence = ?, overridden_by_user = ?, source_fact_id = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(input.preference.entity_id)
    .bind(input.preference.category as i16)
    .bind(&input.preference.key)
    .bind(&input.preference.value)
    .bind(input.preference.confidence)
    .bind(input.preference.overridden_by_user)
    .bind(input.preference.source_fact_id)
    .bind(now)
    .bind(existing_id)
    .execute(&mut **tx)
    .await?;

    // 3. Replace contexts.
    sqlx::query("DELETE FROM preference_contexts WHERE preference_id = ?")
        .bind(existing_id)
        .execute(&mut **tx)
        .await?;

    for (ctx_key, ctx_value) in &input.contexts {
        sqlx::query(
            "INSERT INTO preference_contexts (preference_id, context_key, context_value) \
             VALUES (?, ?, ?)",
        )
        .bind(existing_id)
        .bind(ctx_key)
        .bind(ctx_value)
        .execute(&mut **tx)
        .await?;
    }

    // 4. Replace sources.
    sqlx::query("DELETE FROM preference_sources WHERE preference_id = ?")
        .bind(existing_id)
        .execute(&mut **tx)
        .await?;

    for (source_type, source_id) in &input.sources {
        sqlx::query(
            "INSERT INTO preference_sources (preference_id, source_type_id, source_id, extracted_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(existing_id)
        .bind(*source_type as i16)
        .bind(source_id)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }

    // 5. Return updated preference.
    let pref = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences WHERE id = ?",
    )
    .bind(existing_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(pref)
}

// ---------------------------------------------------------------------------
// Insert
// ---------------------------------------------------------------------------

/// Insert a new preference with transactional provenance, context, and audit logging.
///
/// # Uniqueness
/// Before insert, checks whether an existing preference shares the same
/// `(entity_id, key)` and an identical set of context rows. If so, returns
/// [`KnowledgeError::DuplicatePreference`].
pub async fn insert_preference(
    pool: &SqlitePool,
    input: &UpsertPreferenceInput,
    now: DateTime<Utc>,
) -> Result<Preference, KnowledgeError> {
    // 0. Validate confidence before acquiring a write lock.
    if input.preference.overridden_by_user && input.preference.confidence != 1.0 {
        return Err(KnowledgeError::Validation(format!(
            "explicit preference must have confidence 1.0, got {}",
            input.preference.confidence
        )));
    }
    if !input.preference.overridden_by_user && !(0.0..=1.0).contains(&input.preference.confidence) {
        return Err(KnowledgeError::Validation(format!(
            "confidence must be in [0.0, 1.0], got {}",
            input.preference.confidence
        )));
    }

    let mut tx = pool.begin().await?;
    let pref = insert_preference_in_tx(&mut tx, input, now).await?;
    tx.commit().await?;
    Ok(pref)
}

// ---------------------------------------------------------------------------
// Upsert with conflict resolution
// ---------------------------------------------------------------------------

/// Upsert a preference, applying conflict-resolution rules.
///
/// # Rules
/// 1. Existing `overridden_by_user = true`, new `overridden_by_user = false` → rejected.
/// 2. Existing inferred, new explicit (`overridden_by_user = true`) → overwrite.
/// 3. Both inferred → higher confidence wins.
/// 4. Same confidence → keep existing.
/// 5. Both explicit → new overwrites (user is updating their setting).
pub async fn upsert_preference(
    pool: &SqlitePool,
    input: &UpsertPreferenceInput,
    now: DateTime<Utc>,
) -> Result<(Preference, UpsertAction), KnowledgeError> {
    // 0. Validate confidence before acquiring a write lock.
    if input.preference.overridden_by_user && input.preference.confidence != 1.0 {
        return Err(KnowledgeError::Validation(format!(
            "explicit preference must have confidence 1.0, got {}",
            input.preference.confidence
        )));
    }
    if !input.preference.overridden_by_user && !(0.0..=1.0).contains(&input.preference.confidence) {
        return Err(KnowledgeError::Validation(format!(
            "confidence must be in [0.0, 1.0], got {}",
            input.preference.confidence
        )));
    }

    let mut tx = pool.begin().await?;

    // 1. Look up existing preference with same (entity_id, key) and identical context set.
    let candidates: Vec<Preference> = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences \
         WHERE (entity_id IS ?) AND key = ?",
    )
    .bind(input.preference.entity_id)
    .bind(&input.preference.key)
    .fetch_all(&mut *tx)
    .await?;

    let input_len = input.contexts.len();
    let mut existing: Option<Preference> = None;

    for candidate in candidates {
        let ctx_rows: Vec<PreferenceContext> = sqlx::query_as::<_, PreferenceContext>(
            "SELECT id, preference_id, context_key, context_value \
             FROM preference_contexts \
             WHERE preference_id = ?",
        )
        .bind(candidate.id)
        .fetch_all(&mut *tx)
        .await?;

        if ctx_rows.len() == input_len {
            let candidate_ctx_set: HashSet<(&str, &str)> = ctx_rows
                .iter()
                .map(|c| (c.context_key.as_str(), c.context_value.as_str()))
                .collect();

            if input
                .contexts
                .iter()
                .all(|(k, v)| candidate_ctx_set.contains(&(k.as_str(), v.as_str())))
            {
                existing = Some(candidate);
                break;
            }
        }
    }

    // No existing → create inside the same transaction.
    let existing = match existing {
        None => {
            let pref = insert_preference_in_tx(&mut tx, input, now).await?;
            tx.commit().await?;
            return Ok((pref, UpsertAction::Created));
        }
        Some(e) => e,
    };

    // Conflict resolution.
    let action =
        resolve_conflict(&mut tx, &existing, &input.preference, input.changed_by, now).await?;

    let pref = match action {
        UpsertAction::Overwritten => {
            // Update in-place to preserve preference_id for audit trail.
            update_preference_in_tx(&mut tx, existing.id, input, now).await?
        }
        _ => existing,
    };

    tx.commit().await?;

    Ok((pref, action))
}

async fn resolve_conflict(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    existing: &Preference,
    new: &NewPreference,
    changed_by: ChangedBy,
    now: DateTime<Utc>,
) -> Result<UpsertAction, KnowledgeError> {
    // Rule 1: existing explicit, new inferred → rejected.
    if existing.overridden_by_user && !new.overridden_by_user {
        return Ok(UpsertAction::Rejected);
    }

    // Rule 2: existing inferred, new explicit → overwrite.
    if !existing.overridden_by_user && new.overridden_by_user {
        write_preference_audit_log(
            tx,
            AuditLogParams {
                preference_id: existing.id,
                change_type: ChangeType::ConfidenceChange,
                old_value: Some(&existing.value),
                new_value: Some(&new.value),
                now,
                changed_by,
                reason: Some("overridden by user"),
            },
        )
        .await?;
        return Ok(UpsertAction::Overwritten);
    }

    // Rule 5: both explicit → new wins (user updating their setting).
    if existing.overridden_by_user && new.overridden_by_user {
        write_preference_audit_log(
            tx,
            AuditLogParams {
                preference_id: existing.id,
                change_type: ChangeType::ConfidenceChange,
                old_value: Some(&existing.value),
                new_value: Some(&new.value),
                now,
                changed_by,
                reason: Some("updated by user"),
            },
        )
        .await?;
        return Ok(UpsertAction::Overwritten);
    }

    // Both inferred (or at least new is inferred).
    // Rule 3: higher confidence wins.
    if new.confidence > existing.confidence {
        write_preference_audit_log(
            tx,
            AuditLogParams {
                preference_id: existing.id,
                change_type: ChangeType::ConfidenceChange,
                old_value: Some(&existing.value),
                new_value: Some(&new.value),
                now,
                changed_by,
                reason: Some("higher confidence inferred preference"),
            },
        )
        .await?;
        return Ok(UpsertAction::Overwritten);
    }

    // Rule 4: same or lower confidence → keep existing.
    if new.confidence == existing.confidence {
        write_preference_audit_log(
            tx,
            AuditLogParams {
                preference_id: existing.id,
                change_type: ChangeType::ConfidenceChange,
                old_value: Some(&existing.value),
                new_value: Some(&new.value),
                now,
                changed_by,
                reason: Some("equal confidence inferred preference"),
            },
        )
        .await?;
    }
    Ok(UpsertAction::KeptAsPrimary)
}

/// Input bundle for writing a preference audit-log entry.
struct AuditLogParams<'a> {
    preference_id: i32,
    change_type: ChangeType,
    old_value: Option<&'a str>,
    new_value: Option<&'a str>,
    now: DateTime<Utc>,
    changed_by: ChangedBy,
    reason: Option<&'a str>,
}

async fn write_preference_audit_log(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    params: AuditLogParams<'_>,
) -> Result<(), KnowledgeError> {
    sqlx::query(
        "INSERT INTO preference_audit_log \
         (preference_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(params.preference_id)
    .bind(params.change_type as i16)
    .bind(params.old_value)
    .bind(params.new_value)
    .bind(params.now)
    .bind(params.changed_by as i16)
    .bind(params.reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Contextual lookup
// ---------------------------------------------------------------------------
