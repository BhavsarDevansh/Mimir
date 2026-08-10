//! Preference read paths: lookup by entity+key, by id, by context/source,
//! and audit-trail queries.

use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

use crate::KnowledgeError;
use crate::models::preference::{
    Preference, PreferenceAuditLogEntry, PreferenceContext, PreferenceSource,
};

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
