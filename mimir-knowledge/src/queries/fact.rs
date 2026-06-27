//! Fact CRUD, temporal queries, overlap logic, and audit logging.

use chrono::{DateTime, Utc};
use serde_json;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};

/// Predicates that represent a collection of independent values.
/// Facts with these predicates and different objects should coexist.
pub const MULTI_VALUED_PREDICATES: [&str; 11] = [
    "hobby",
    "likes",
    "dislikes",
    "favourite_colour",
    "favourite_food",
    "skill",
    "has_pets",
    "has_child",
    "has_parent",
    "has_sibling",
    "has_partner",
];
use crate::models::enums::RelationType;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, Source, SourceType};

// ---------------------------------------------------------------------------
// Corroboration constants (#79)
// ---------------------------------------------------------------------------

/// Confidence gained per independent corroborating source.
const CORROBORATION_BOOST: f32 = 0.05;

/// Upper bound for non-explicit fact confidence (explicit facts use 1.0).
const NON_EXPLICIT_CONFIDENCE_CAP: f32 = 0.95;

#[allow(clippy::too_many_arguments)]
/// Update multiple mutable fields on a fact in a single transaction.
/// Writes an audit entry per changed field.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_extraction_method(source_type: SourceType) -> Option<i16> {
    match source_type {
        SourceType::UserEdit => Some(ExtractionMethod::UserInput as i16),
        SourceType::Connector => Some(ExtractionMethod::StructuredParse as i16),
        SourceType::Inference => Some(ExtractionMethod::InferenceRule as i16),
        SourceType::Interaction => Some(ExtractionMethod::LlmExtraction as i16),
        SourceType::Import => Some(ExtractionMethod::StructuredParse as i16),
        SourceType::System => None,
    }
}

fn changed_by_for_source_type(source_type: SourceType) -> ChangedBy {
    match source_type {
        SourceType::UserEdit => ChangedBy::User,
        SourceType::Connector => ChangedBy::System,
        SourceType::Inference => ChangedBy::InferenceEngine,
        SourceType::Interaction => ChangedBy::System,
        SourceType::Import => ChangedBy::User,
        SourceType::System => ChangedBy::System,
    }
}

/// Fetch a single fact by id within an in-flight transaction.
async fn fact_by_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fact_id: i32,
) -> Result<Fact, KnowledgeError> {
    sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(KnowledgeError::from)
}

/// Whether `ef` has the same object as the given new-fact object fields.
fn same_object_as(new_object_id: Option<i32>, new_object_literal: Option<&str>, ef: &Fact) -> bool {
    match (new_object_id, new_object_literal) {
        (Some(new_oid), _) => ef.object_id == Some(new_oid),
        (None, Some(new_lit)) => ef.object_literal.as_deref() == Some(new_lit),
        (None, None) => ef.object_id.is_none() && ef.object_literal.is_none(),
    }
}

// ---------------------------------------------------------------------------
// Insert
// ---------------------------------------------------------------------------

/// Insert a new fact with transactional provenance and temporal overlap handling.
///
/// Temporal rules (same `subject_id + relationship_type_id`):
/// - Non-overlapping ranges → `Active`
/// - Overlapping with both unbounded → `Disputed`
/// - Old open-ended + new explicit starting now → close old at `now()`, new `Active`
/// - Any other overlap → `Disputed`
///
/// `relationship_type_id` and `confidence` are resolved by the caller (`KnowledgeGraph`).
pub async fn insert_fact(
    pool: &SqlitePool,
    new_fact: &NewFact,
    relationship_type_id: i16,
    confidence: f32,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;
    let fact = insert_fact_in_tx(
        &mut tx,
        new_fact,
        relationship_type_id,
        &new_fact.relationship_type,
        confidence,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(fact)
}

pub async fn insert_fact_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    new_fact: &NewFact,
    relationship_type_id: i16,
    relationship_type_name: &str,
    confidence: f32,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    // 0. Validate time range ordering.
    if let (Some(from), Some(until)) = (new_fact.valid_from, new_fact.valid_until) {
        if from > until {
            return Err(KnowledgeError::Validation(format!(
                "valid_from ({}) must not be after valid_until ({})",
                from, until
            )));
        }
    }

    // 1. Temporal overlap check against same subject + predicate.
    let existing: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ?",
    )
    .bind(new_fact.subject_id)
    .bind(relationship_type_id)
    .fetch_all(&mut **tx)
    .await?;

    // Collect all overlapping facts.
    // For multi-valued predicates (e.g. hobby, has_sibling), facts with
    // different objects are independent and should not supersede each other.
    let is_multi_valued = MULTI_VALUED_PREDICATES.contains(&relationship_type_name);
    let overlaps: Vec<&Fact> = existing
        .iter()
        .filter(|ef| {
            let same_object =
                same_object_as(new_fact.object_id, new_fact.object_literal.as_deref(), ef);
            if !same_object && is_multi_valued {
                return false;
            }
            ranges_overlap(
                ef.valid_from,
                ef.valid_until,
                new_fact.valid_from,
                new_fact.valid_until,
            )
        })
        .collect();

    // --- Corroboration detection (#79) ---
    // A new non-explicit fact covering the same claim as an existing
    // Active/pending fact (same object, temporally overlapping) corroborates
    // it: a source row is added to the existing fact and its confidence is
    // boosted (+0.05, capped at 0.95 for non-explicit, non-inferred facts).
    // No new facts row is created. This runs *before* the supersession path
    // below, so an explicit statement still supersedes rather than
    // corroborates. An identical re-statement (non-independent source) is a
    // no-op to avoid colliding with the sources UNIQUE index.
    if new_fact.source_type != SourceType::UserEdit {
        let candidate = overlaps.iter().find(|ef| {
            if !same_object_as(new_fact.object_id, new_fact.object_literal.as_deref(), ef) {
                return false;
            }
            ef.status() == Some(FactStatus::Active) || ef.pending_confirmation
        });

        if let Some(existing_fact) = candidate {
            let connector_id_ref = new_fact.connector_id.as_deref();
            let raw_ref = new_fact.raw_reference.as_deref();

            // Independence check: a source with identical provenance already
            // recorded against this fact is a re-statement, not corroboration.
            let already: Option<(i64,)> = sqlx::query_as(
                "SELECT 1 FROM sources \
                 WHERE fact_id = ? AND source_type_id = ? \
                 AND COALESCE(connector_id, '') = COALESCE(?, '') \
                 AND COALESCE(raw_reference, '') = COALESCE(?, '') \
                 LIMIT 1",
            )
            .bind(existing_fact.id)
            .bind(new_fact.source_type as i16)
            .bind(connector_id_ref)
            .bind(raw_ref)
            .fetch_optional(&mut **tx)
            .await?;

            if already.is_some() {
                // Duplicate re-statement: return the existing fact unchanged.
                return fact_by_id_in_tx(tx, existing_fact.id).await;
            }

            // Insert the corroborating source against the existing fact.
            let extraction_method_id = new_fact
                .extraction_method
                .map(|e| e as i16)
                .or_else(|| default_extraction_method(new_fact.source_type));
            let connector_type_id = new_fact.connector_type.map(|ct| ct as i16);

            let source_id: i64 = sqlx::query_scalar(
                "INSERT INTO sources \
                 (fact_id, source_type_id, connector_id, connector_type_id, raw_reference, extracted_at, extraction_method_id) \
                 VALUES (?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id",
            )
            .bind(existing_fact.id)
            .bind(new_fact.source_type as i16)
            .bind(&new_fact.connector_id)
            .bind(connector_type_id)
            .bind(&new_fact.raw_reference)
            .bind(now)
            .bind(extraction_method_id)
            .fetch_one(&mut **tx)
            .await?;
            let source_id = source_id as i32;

            // SourceAdded audit entry (mirrors queries::source::add_source_to_fact).
            let source_added_value = serde_json::json!({
                "source_type_id": new_fact.source_type as i16,
                "connector_id": new_fact.connector_id,
                "connector_type_id": connector_type_id,
                "raw_reference": new_fact.raw_reference,
                "extraction_method_id": extraction_method_id,
            })
            .to_string();
            sqlx::query(
                "INSERT INTO fact_audit_log \
                 (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(existing_fact.id)
            .bind(ChangeType::SourceAdded as i16)
            .bind(None::<&str>)
            .bind(&source_added_value)
            .bind(now)
            .bind(changed_by_for_source_type(new_fact.source_type) as i16)
            .bind(None::<&str>)
            .execute(&mut **tx)
            .await?;

            // Confidence boost applies only to non-explicit, non-inferred
            // facts. Explicit (UserEdit/System) facts stay at 1.0; inferred
            // fact confidence is structural (recalculated from parents).
            let explicit: Option<(i64,)> = sqlx::query_as(
                "SELECT 1 FROM sources \
                 WHERE fact_id = ? AND source_type_id IN (?, ?) LIMIT 1",
            )
            .bind(existing_fact.id)
            .bind(SourceType::UserEdit as i16)
            .bind(SourceType::System as i16)
            .fetch_optional(&mut **tx)
            .await?;

            let can_boost = !existing_fact.inferred && explicit.is_none();

            if can_boost {
                let new_confidence = (existing_fact.confidence + CORROBORATION_BOOST)
                    .min(NON_EXPLICIT_CONFIDENCE_CAP);
                if (new_confidence - existing_fact.confidence).abs() > 1e-6 {
                    let old_json =
                        serde_json::json!({"confidence": existing_fact.confidence}).to_string();
                    let new_json = serde_json::json!({
                        "confidence": new_confidence,
                        "source_id": source_id,
                    })
                    .to_string();
                    sqlx::query(
                        "UPDATE facts SET confidence = ?, stale_confidence = FALSE, updated_at = ? WHERE id = ?",
                    )
                    .bind(new_confidence)
                    .bind(now)
                    .bind(existing_fact.id)
                    .execute(&mut **tx)
                    .await?;

                    sqlx::query(
                        "INSERT INTO fact_audit_log \
                         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(existing_fact.id)
                    .bind(ChangeType::ConfidenceChange as i16)
                    .bind(&old_json)
                    .bind(&new_json)
                    .bind(now)
                    .bind(ChangedBy::System as i16)
                    .bind(None::<&str>)
                    .execute(&mut **tx)
                    .await?;

                    // Cascade the confidence change to all inferred children,
                    // comprehensively, within this transaction.
                    crate::confidence::cascade_confidence_change_in_tx(tx, existing_fact.id, now)
                        .await?;
                }
            }

            return fact_by_id_in_tx(tx, existing_fact.id).await;
        }
    }

    let mut fact_status = FactStatus::Active;
    let mut facts_to_supersede: Vec<i32> = Vec::new();
    let mut contradicts_pairs: Vec<i32> = Vec::new();

    if !overlaps.is_empty() {
        if new_fact.source_type == SourceType::UserEdit {
            // Explicit replacement: supersede all overlapping facts.
            for existing_fact in &overlaps {
                // Temporal closure for sole open-ended predecessor.
                let is_sole_open = overlaps.len() == 1
                    && existing_fact.valid_until.is_none()
                    && new_fact.valid_from.is_some();

                if is_sole_open {
                    let new_start = new_fact.valid_from.unwrap();
                    let old_json =
                        serde_json::json!({"valid_until": existing_fact.valid_until}).to_string();
                    sqlx::query("UPDATE facts SET valid_until = ?, updated_at = ? WHERE id = ?")
                        .bind(new_start)
                        .bind(now)
                        .bind(existing_fact.id)
                        .execute(&mut **tx)
                        .await?;

                    let updated: Fact = sqlx::query_as::<_, Fact>(
                        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                         valid_from, valid_until, confidence, fact_status_id, inferred, \
                         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
                         FROM facts WHERE id = ?",
                    )
                    .bind(existing_fact.id)
                    .fetch_one(&mut **tx)
                    .await?;

                    let new_json =
                        serde_json::json!({"valid_until": updated.valid_until}).to_string();
                    sqlx::query(
                        "INSERT INTO fact_audit_log \
                         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(existing_fact.id)
                    .bind(ChangeType::TemporalUpdate as i16)
                    .bind(old_json)
                    .bind(new_json)
                    .bind(now)
                    .bind(ChangedBy::System as i16)
                    .bind(None::<&str>)
                    .execute(&mut **tx)
                    .await?;
                }

                // Mark as Superseded unless already superseded.
                if existing_fact.status() != Some(FactStatus::Superseded) {
                    let old_json =
                        serde_json::json!({"fact_status_id": existing_fact.fact_status_id})
                            .to_string();
                    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
                        .bind(FactStatus::Superseded as i16)
                        .bind(now)
                        .bind(existing_fact.id)
                        .execute(&mut **tx)
                        .await?;

                    let updated: Fact = sqlx::query_as::<_, Fact>(
                        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                         valid_from, valid_until, confidence, fact_status_id, inferred, \
                         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
                         FROM facts WHERE id = ?",
                    )
                    .bind(existing_fact.id)
                    .fetch_one(&mut **tx)
                    .await?;

                    let new_json =
                        serde_json::json!({"fact_status_id": updated.fact_status_id}).to_string();
                    sqlx::query(
                        "INSERT INTO fact_audit_log \
                         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(existing_fact.id)
                    .bind(ChangeType::StatusChange as i16)
                    .bind(old_json)
                    .bind(new_json)
                    .bind(now)
                    .bind(ChangedBy::System as i16)
                    .bind(None::<&str>)
                    .execute(&mut **tx)
                    .await?;

                    facts_to_supersede.push(existing_fact.id);
                }
            }
        } else {
            // Overlap with non-explicit source → mark new fact as Disputed
            // and also mark existing overlapping facts as Disputed.
            fact_status = FactStatus::Disputed;
            for existing_fact in &overlaps {
                // Skip Superseded and Forgotten facts; they should not be resurrected.
                if existing_fact.status() == Some(FactStatus::Superseded)
                    || existing_fact.status() == Some(FactStatus::Forgotten)
                {
                    continue;
                }
                if existing_fact.fact_status_id != FactStatus::Disputed as i16 {
                    let old_json =
                        serde_json::json!({"fact_status_id": existing_fact.fact_status_id})
                            .to_string();
                    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
                        .bind(FactStatus::Disputed as i16)
                        .bind(now)
                        .bind(existing_fact.id)
                        .execute(&mut **tx)
                        .await?;

                    let new_json =
                        serde_json::json!({"fact_status_id": FactStatus::Disputed as i16})
                            .to_string();
                    sqlx::query(
                        "INSERT INTO fact_audit_log \
                         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(existing_fact.id)
                    .bind(ChangeType::StatusChange as i16)
                    .bind(old_json)
                    .bind(new_json)
                    .bind(now)
                    .bind(ChangedBy::System as i16)
                    .bind(None::<&str>)
                    .execute(&mut **tx)
                    .await?;
                }
                contradicts_pairs.push(existing_fact.id);
            }
        }
    }

    // 2. Resolve memory priority from relationship type.
    let memory_priority_id: i16 = sqlx::query_scalar(
        "SELECT COALESCE(r.default_memory_priority_id, p.id) \
         FROM relationship_types r \
         CROSS JOIN memory_priorities p \
         WHERE r.id = ? AND p.name = 'Normal'",
    )
    .bind(relationship_type_id)
    .fetch_one(&mut **tx)
    .await?;

    // 3. Insert the fact.
    let fact_id: i64 = sqlx::query_scalar(
        "INSERT INTO facts \
         (subject_id, relationship_type_id, object_id, object_literal, valid_from, valid_until, \
          confidence, fact_status_id, inferred, inference_depth, pending_confirmation, memory_priority_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(new_fact.subject_id)
    .bind(relationship_type_id)
    .bind(new_fact.object_id)
    .bind(&new_fact.object_literal)
    .bind(new_fact.valid_from)
    .bind(new_fact.valid_until)
    .bind(confidence)
    .bind(fact_status as i16)
    .bind(new_fact.inferred)
    .bind(new_fact.inference_depth)
    .bind(false)
    .bind(memory_priority_id)
    .bind(now)
    .bind(now)
    .fetch_one(&mut **tx)
    .await?;

    let fact_id = fact_id as i32;

    // 4. Resolve extraction method.
    let extraction_method_id = new_fact
        .extraction_method
        .map(|e| e as i16)
        .or_else(|| default_extraction_method(new_fact.source_type));

    // 5. Insert the source row.
    let connector_type_id = new_fact.connector_type.map(|ct| ct as i16);
    sqlx::query(
        "INSERT INTO sources \
         (fact_id, source_type_id, connector_id, connector_type_id, raw_reference, extracted_at, extraction_method_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(new_fact.source_type as i16)
    .bind(&new_fact.connector_id)
    .bind(connector_type_id)
    .bind(&new_fact.raw_reference)
    .bind(now)
    .bind(extraction_method_id)
    .execute(&mut **tx)
    .await?;

    // 6. Write created audit entry (column-only snapshot).
    let new_value = serde_json::json!({
        "fact_id": fact_id,
        "confidence": confidence,
        "fact_status_id": fact_status as i16,
        "valid_from": new_fact.valid_from,
        "valid_until": new_fact.valid_until,
    })
    .to_string();

    sqlx::query(
        "INSERT INTO fact_audit_log \
         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(ChangeType::Created as i16)
    .bind(None::<&str>)
    .bind(new_value)
    .bind(now)
    .bind(changed_by_for_source_type(new_fact.source_type) as i16)
    .bind(None::<&str>)
    .execute(&mut **tx)
    .await?;

    // 6. Insert superseded edges for any facts replaced by a user edit.
    for existing_id in facts_to_supersede {
        sqlx::query(
            "INSERT INTO fact_dependencies \
             (parent_fact_id, child_fact_id, relation_type_id, is_positive) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(existing_id)
        .bind(fact_id)
        .bind(RelationType::Supersedes as i16)
        .bind(true)
        .execute(&mut **tx)
        .await?;
    }

    // 7. Insert Contradicts edges in both directions for disputed overlaps.
    for existing_id in contradicts_pairs {
        sqlx::query(
            "INSERT INTO fact_dependencies \
             (parent_fact_id, child_fact_id, relation_type_id, is_positive) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(existing_id)
        .bind(fact_id)
        .bind(RelationType::Contradicts as i16)
        .bind(false)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            "INSERT INTO fact_dependencies \
             (parent_fact_id, child_fact_id, relation_type_id, is_positive) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(fact_id)
        .bind(existing_id)
        .bind(RelationType::Contradicts as i16)
        .bind(false)
        .execute(&mut **tx)
        .await?;
    }

    // 9. Return the inserted fact.
    let fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(fact)
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Get a fact by ID.
pub async fn get_by_id(pool: &SqlitePool, fact_id: i32) -> Result<Option<Fact>, KnowledgeError> {
    let fact: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(pool)
    .await?;

    Ok(fact)
}

/// List facts for a subject entity.
pub async fn get_by_subject(
    pool: &SqlitePool,
    subject_id: i32,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE subject_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(subject_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

/// List facts for a predicate.
pub async fn get_by_predicate(
    pool: &SqlitePool,
    relationship_type_id: i16,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE relationship_type_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(relationship_type_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

/// List facts for an object entity.
pub async fn get_by_object(
    pool: &SqlitePool,
    object_id: i32,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE object_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(object_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

/// Return facts active at a specific point in time.
pub async fn get_active_facts_at(
    pool: &SqlitePool,
    subject_id: i32,
    relationship_type_id: i16,
    at: DateTime<Utc>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ? \
           AND fact_status_id = ? \
           AND (valid_from IS NULL OR valid_from <= ?) \
           AND (valid_until IS NULL OR valid_until > ?) \
         ORDER BY valid_from",
    )
    .bind(subject_id)
    .bind(relationship_type_id)
    .bind(FactStatus::Active as i16)
    .bind(at)
    .bind(at)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Update a fact's `valid_until`, writing a `temporal_update` audit entry.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Determine whether two optional time ranges overlap.
///
/// A range is `[from, until)` where `None` means unbounded on that side.
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

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

/// Retrieve audit log entries for a given fact, newest first.
pub async fn get_audit_log(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Vec<crate::models::audit_log::AuditLogEntry>, KnowledgeError> {
    let entries: Vec<crate::models::audit_log::AuditLogEntry> =
        sqlx::query_as::<_, crate::models::audit_log::AuditLogEntry>(
            "SELECT id, fact_id, change_type_id, old_value, new_value, \
             changed_at, changed_by_id, reason \
             FROM fact_audit_log \
             WHERE fact_id = ? \
             ORDER BY changed_at DESC",
        )
        .bind(fact_id)
        .fetch_all(pool)
        .await?;
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Filtered fact queries for tool layer
// ---------------------------------------------------------------------------

/// A fact row joined with the object entity name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FactWithObjectName {
    pub id: i32,
    pub subject_id: i32,
    pub relationship_type_id: i16,
    pub object_id: Option<i32>,
    pub object_literal: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub fact_status_id: i16,
    pub inferred: bool,
    pub inference_depth: i32,
    pub stale_confidence: bool,
    pub pending_confirmation: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub object_name: Option<String>,
}

/// A fact enriched with its object name and source records.
#[derive(Debug, Clone)]
pub struct FactWithSources {
    pub id: i32,
    pub subject_id: i32,
    pub relationship_type_id: i16,
    pub object_id: Option<i32>,
    pub object_literal: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub fact_status_id: i16,
    pub inferred: bool,
    pub inference_depth: i32,
    pub stale_confidence: bool,
    pub pending_confirmation: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub object_name: Option<String>,
    pub sources: Vec<Source>,
}

/// Batch-fetch sources for the given fact rows and assemble enriched
/// `FactWithSources` records (object name + sources), preserving row order.
async fn enrich_with_sources(
    pool: &SqlitePool,
    rows: Vec<FactWithObjectName>,
) -> Result<Vec<FactWithSources>, KnowledgeError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let fact_ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    let placeholders: Vec<&str> = fact_ids.iter().map(|_| "?").collect();
    let src_sql = format!(
        "SELECT id, fact_id, source_type_id, connector_id, connector_type_id, raw_reference, extracted_at, extraction_method_id \
         FROM sources \
         WHERE fact_id IN ({})",
        placeholders.join(",")
    );
    let mut src_query = sqlx::query_as::<_, Source>(sqlx::AssertSqlSafe(&*src_sql));
    for &id in &fact_ids {
        src_query = src_query.bind(id);
    }
    let sources = src_query.fetch_all(pool).await?;

    let mut sources_by_fact: std::collections::HashMap<i32, Vec<Source>> =
        std::collections::HashMap::new();
    for src in sources {
        sources_by_fact.entry(src.fact_id).or_default().push(src);
    }

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let srcs = sources_by_fact.remove(&row.id).unwrap_or_default();
        results.push(FactWithSources {
            id: row.id,
            subject_id: row.subject_id,
            relationship_type_id: row.relationship_type_id,
            object_id: row.object_id,
            object_literal: row.object_literal,
            valid_from: row.valid_from,
            valid_until: row.valid_until,
            confidence: row.confidence,
            fact_status_id: row.fact_status_id,
            inferred: row.inferred,
            inference_depth: row.inference_depth,
            stale_confidence: row.stale_confidence,
            pending_confirmation: row.pending_confirmation,
            created_at: row.created_at,
            updated_at: row.updated_at,
            object_name: row.object_name,
            sources: srcs,
        });
    }

    Ok(results)
}

/// Retrieve facts for a subject with optional predicate filter and confidence threshold.
pub async fn get_facts_by_subject_filtered(
    pool: &SqlitePool,
    subject_id: i32,
    relationship_type_id_opt: Option<i16>,
    min_confidence: f32,
    offset: i64,
    limit: i64,
) -> Result<Vec<FactWithSources>, KnowledgeError> {
    let rows: Vec<FactWithObjectName> = if let Some(relationship_type_id) = relationship_type_id_opt
    {
        sqlx::query_as::<_, FactWithObjectName>(
            "SELECT f.id, f.subject_id, f.relationship_type_id, f.object_id, f.object_literal, \
                    f.valid_from, f.valid_until, f.confidence, f.fact_status_id, f.inferred, \
                    f.inference_depth, f.stale_confidence, f.pending_confirmation, f.created_at, f.updated_at, \
                    e.name as object_name \
             FROM facts f \
             LEFT JOIN entities e ON e.id = f.object_id \
             WHERE f.subject_id = ? \
               AND f.pending_confirmation = 0 \
               AND f.fact_status_id NOT IN (5, 6) \
               AND f.relationship_type_id = ? \
               AND f.confidence >= ? \
             ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC \
             LIMIT ? OFFSET ?",
        )
        .bind(subject_id)
        .bind(relationship_type_id)
        .bind(min_confidence)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, FactWithObjectName>(
            "SELECT f.id, f.subject_id, f.relationship_type_id, f.object_id, f.object_literal, \
                    f.valid_from, f.valid_until, f.confidence, f.fact_status_id, f.inferred, \
                    f.inference_depth, f.stale_confidence, f.pending_confirmation, f.created_at, f.updated_at, \
                    e.name as object_name \
             FROM facts f \
             LEFT JOIN entities e ON e.id = f.object_id \
             WHERE f.subject_id = ? \
               AND f.pending_confirmation = 0 \
               AND f.fact_status_id NOT IN (5, 6) \
               AND f.confidence >= ? \
             ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC \
             LIMIT ? OFFSET ?",
        )
        .bind(subject_id)
        .bind(min_confidence)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };

    enrich_with_sources(pool, rows).await
}

/// Count facts for a subject with optional predicate filter and confidence threshold.
pub async fn count_facts_by_subject_filtered(
    pool: &SqlitePool,
    subject_id: i32,
    relationship_type_id_opt: Option<i16>,
    min_confidence: f32,
) -> Result<i64, KnowledgeError> {
    let count: i64 = if let Some(relationship_type_id) = relationship_type_id_opt {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM facts \
             WHERE subject_id = ? \
               AND pending_confirmation = 0 \
               AND fact_status_id NOT IN (5, 6) \
               AND relationship_type_id = ? \
               AND confidence >= ?",
        )
        .bind(subject_id)
        .bind(relationship_type_id)
        .bind(min_confidence)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM facts \
             WHERE subject_id = ? \
               AND pending_confirmation = 0 \
               AND fact_status_id NOT IN (5, 6) \
               AND confidence >= ?",
        )
        .bind(subject_id)
        .bind(min_confidence)
        .fetch_one(pool)
        .await?
    };
    Ok(count)
}

/// Recursive CTE yielding a relationship type and all of its descendants in the
/// `relationship_type_hierarchy` DAG. The first bound parameter is the root type
/// id; `UNION` (not `UNION ALL`) deduplicates ids reachable via multiple paths.
const RELATIONSHIP_SUBTREE_CTE: &str = "WITH RECURSIVE subtree(id) AS ( \
    SELECT ? \
    UNION \
    SELECT h.child_id FROM relationship_type_hierarchy h \
    JOIN subtree s ON h.parent_id = s.id \
)";

/// Retrieve facts for a subject whose relationship type is `root_type_id` or
/// any descendant in the `relationship_type_hierarchy` DAG.
///
/// Walks the DAG via a single SQLite recursive CTE that seeds with the root
/// type itself, then unions all children. Filters to non-pending facts whose
/// status is not Superseded or Forgotten (`NOT IN (5, 6)`), with confidence at
/// least `min_confidence`, sorted by confidence descending (then `valid_from`
/// descending, then `id` descending). Enriched with the object entity name and
/// batched source records via [`enrich_with_sources`].
pub async fn get_facts_by_relationship_subtree(
    pool: &SqlitePool,
    subject_id: i32,
    root_type_id: i16,
    min_confidence: f32,
    limit: i64,
) -> Result<Vec<FactWithSources>, KnowledgeError> {
    let sql = format!(
        "{RELATIONSHIP_SUBTREE_CTE} \
         SELECT f.id, f.subject_id, f.relationship_type_id, f.object_id, f.object_literal, \
                f.valid_from, f.valid_until, f.confidence, f.fact_status_id, f.inferred, \
                f.inference_depth, f.stale_confidence, f.pending_confirmation, f.created_at, f.updated_at, \
                e.name as object_name \
         FROM facts f \
         JOIN subtree s ON f.relationship_type_id = s.id \
         LEFT JOIN entities e ON e.id = f.object_id \
         WHERE f.subject_id = ? \
           AND f.pending_confirmation = 0 \
           AND f.fact_status_id NOT IN (5, 6) \
           AND f.confidence >= ? \
         ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC \
         LIMIT ?"
    );
    let rows: Vec<FactWithObjectName> =
        sqlx::query_as::<_, FactWithObjectName>(sqlx::AssertSqlSafe(&*sql))
            .bind(root_type_id)
            .bind(subject_id)
            .bind(min_confidence)
            .bind(limit)
            .fetch_all(pool)
            .await?;

    enrich_with_sources(pool, rows).await
}

/// Count facts for a subject whose relationship type is `root_type_id` or any
/// descendant, applying the same filters as
/// [`get_facts_by_relationship_subtree`] (non-pending, status `NOT IN (5, 6)`,
/// confidence at least `min_confidence`).
pub async fn count_facts_by_relationship_subtree(
    pool: &SqlitePool,
    subject_id: i32,
    root_type_id: i16,
    min_confidence: f32,
) -> Result<i64, KnowledgeError> {
    let sql = format!(
        "{RELATIONSHIP_SUBTREE_CTE} \
         SELECT COUNT(*) \
         FROM facts f \
         JOIN subtree s ON f.relationship_type_id = s.id \
         WHERE f.subject_id = ? \
           AND f.pending_confirmation = 0 \
           AND f.fact_status_id NOT IN (5, 6) \
           AND f.confidence >= ?"
    );
    let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(&*sql))
        .bind(root_type_id)
        .bind(subject_id)
        .bind(min_confidence)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// Pending sensitive-fact confirmation queries
// ---------------------------------------------------------------------------

/// A pending sensitive fact with resolved subject, predicate, and object names.
///
/// Used by the confirmation lifecycle surface (`GET /kb/pending`).
#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
pub struct PendingFactRow {
    pub fact_id: i32,
    pub subject: String,
    pub predicate: String,
    /// Resolved object: entity name when the object is an entity, else the
    /// literal value.
    pub object: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// List all facts awaiting user confirmation (`pending_confirmation = TRUE`),
/// oldest first, joined to resolve human-readable subject, predicate, and
/// object names.
pub async fn list_pending(pool: &SqlitePool) -> Result<Vec<PendingFactRow>, KnowledgeError> {
    let rows: Vec<PendingFactRow> = sqlx::query_as::<_, PendingFactRow>(
        "SELECT f.id AS fact_id, \
                s.name AS subject, \
                rt.name AS predicate, \
                COALESCE(o.name, f.object_literal) AS object, \
                f.created_at AS created_at \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         WHERE f.pending_confirmation = TRUE \
         ORDER BY f.created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
