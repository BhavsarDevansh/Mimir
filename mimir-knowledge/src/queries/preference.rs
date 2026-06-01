//! Preference CRUD, contextual lookup, conflict resolution, and audit logging.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::preference::{
    NewPreference, Preference, PreferenceAuditLogEntry, PreferenceContext, PreferenceSource,
    UpsertAction, UpsertPreferenceInput,
};

// ---------------------------------------------------------------------------
// Insert (internal helper that works inside an existing transaction)
// ---------------------------------------------------------------------------

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
    // 0. Validate confidence range before acquiring a write lock.
    if !(0.0..=1.0).contains(&input.preference.confidence) {
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
    // 0. Validate confidence range before acquiring a write lock.
    if !(0.0..=1.0).contains(&input.preference.confidence) {
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
            // Delete old preference (cascades to contexts, sources).
            sqlx::query("DELETE FROM preferences WHERE id = ?")
                .bind(existing.id)
                .execute(&mut *tx)
                .await?;

            // Insert the new preference atomically in the same transaction.
            insert_preference_in_tx(&mut tx, input, now).await?
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

/// Retrieve the best-matching preference for `(entity_id, key)` given a set of
/// context conditions.
///
/// Ranking:
/// 1. Match count (how many context rows match the query) — descending.
/// 2. If no preference has matching contexts, the preference with zero context
///    rows (the default) is returned.
/// 3. Confidence — descending.
/// 4. `updated_at` — descending.
///
/// Returns `None` if no rows exist at all.
pub async fn get_preference(
    pool: &SqlitePool,
    entity_id: Option<i32>,
    key: &str,
    query_context: &[(String, String)],
) -> Result<Option<Preference>, KnowledgeError> {
    let prefs: Vec<Preference> = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences \
         WHERE (entity_id IS ?) AND key = ?",
    )
    .bind(entity_id)
    .bind(key)
    .fetch_all(pool)
    .await?;

    if prefs.is_empty() {
        return Ok(None);
    }

    // Fetch all contexts for these preferences in a single query.
    let all_ctx_rows: Vec<PreferenceContext> = sqlx::query_as::<_, PreferenceContext>(
        "SELECT id, preference_id, context_key, context_value \
         FROM preference_contexts \
         WHERE preference_id IN (SELECT id FROM preferences WHERE (entity_id IS ?) AND key = ?)",
    )
    .bind(entity_id)
    .bind(key)
    .fetch_all(pool)
    .await?;

    let mut contexts_by_pref: HashMap<i32, Vec<PreferenceContext>> = HashMap::new();
    for row in all_ctx_rows {
        contexts_by_pref
            .entry(row.preference_id)
            .or_default()
            .push(row);
    }

    let query_set: HashSet<(&str, &str)> = query_context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut scored: Vec<(Preference, usize, usize)> = Vec::with_capacity(prefs.len());

    for pref in prefs {
        let ctx_rows = contexts_by_pref.get(&pref.id);
        let match_count = ctx_rows.map_or(0, |rows| {
            rows.iter()
                .filter(|c| query_set.contains(&(c.context_key.as_str(), c.context_value.as_str())))
                .count()
        });
        let ctx_count = ctx_rows.map_or(0, |rows| rows.len());

        scored.push((pref, match_count, ctx_count));
    }

    // Rank by match count desc, then default preference on zero-match tie,
    // then confidence desc, then updated_at desc.
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| {
                if a.1 == 0 && b.1 == 0 {
                    // No matches: prefer fewer context rows (default over specific).
                    a.2.cmp(&b.2)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| {
                b.0.confidence
                    .partial_cmp(&a.0.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.0.updated_at.cmp(&a.0.updated_at))
    });

    Ok(Some(scored.into_iter().next().unwrap().0))
}

// ---------------------------------------------------------------------------
// Get by ID
// ---------------------------------------------------------------------------

/// Fetch a single preference by ID.
pub async fn get_preference_by_id(
    pool: &SqlitePool,
    id: i32,
) -> Result<Option<Preference>, KnowledgeError> {
    let pref: Option<Preference> = sqlx::query_as::<_, Preference>(
        "SELECT id, entity_id, category_id, key, value, confidence, \
         overridden_by_user, source_fact_id, created_at, updated_at \
         FROM preferences WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(pref)
}

// ---------------------------------------------------------------------------
// Get contexts
// ---------------------------------------------------------------------------

/// Fetch all context rows for a preference.
pub async fn get_contexts_for_preference(
    pool: &SqlitePool,
    preference_id: i32,
) -> Result<Vec<PreferenceContext>, KnowledgeError> {
    let rows = sqlx::query_as::<_, PreferenceContext>(
        "SELECT id, preference_id, context_key, context_value \
         FROM preference_contexts \
         WHERE preference_id = ? \
         ORDER BY context_key",
    )
    .bind(preference_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

// ---------------------------------------------------------------------------
// Get sources
// ---------------------------------------------------------------------------

/// Fetch all source rows for a preference.
pub async fn get_sources_for_preference(
    pool: &SqlitePool,
    preference_id: i32,
) -> Result<Vec<PreferenceSource>, KnowledgeError> {
    let rows = sqlx::query_as::<_, PreferenceSource>(
        "SELECT id, preference_id, source_type_id, source_id, extracted_at \
         FROM preference_sources \
         WHERE preference_id = ? \
         ORDER BY extracted_at",
    )
    .bind(preference_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

// ---------------------------------------------------------------------------
// Get audit log
// ---------------------------------------------------------------------------

/// Fetch all audit-log entries for a preference.
pub async fn get_preference_audit_log(
    pool: &SqlitePool,
    preference_id: i32,
) -> Result<Vec<PreferenceAuditLogEntry>, KnowledgeError> {
    let rows = sqlx::query_as::<_, PreferenceAuditLogEntry>(
        "SELECT id, preference_id, change_type_id, old_value, new_value, \
         changed_at, changed_by_id, reason \
         FROM preference_audit_log \
         WHERE preference_id = ? \
         ORDER BY changed_at DESC",
    )
    .bind(preference_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
