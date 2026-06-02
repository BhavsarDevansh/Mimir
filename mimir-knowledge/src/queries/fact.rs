//! Fact CRUD, temporal queries, overlap logic, and audit logging.

use chrono::{DateTime, Utc};
use serde_json;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::enums::RelationType;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};

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

// ---------------------------------------------------------------------------
// Insert
// ---------------------------------------------------------------------------

/// Insert a new fact with transactional provenance and temporal overlap handling.
///
/// Temporal rules (same `subject_id + predicate_id`):
/// - Non-overlapping ranges → `Active`
/// - Overlapping with both unbounded → `Disputed`
/// - Old open-ended + new explicit starting now → close old at `now()`, new `Active`
/// - Any other overlap → `Disputed`
///
/// `predicate_id` and `confidence` are resolved by the caller (`KnowledgeGraph`).
pub async fn insert_fact(
    pool: &SqlitePool,
    new_fact: &NewFact,
    predicate_id: i16,
    confidence: f32,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;

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
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND predicate_id = ?",
    )
    .bind(new_fact.subject_id)
    .bind(predicate_id)
    .fetch_all(&mut *tx)
    .await?;

    // Collect all overlapping facts.
    let overlaps: Vec<&Fact> = existing
        .iter()
        .filter(|ef| {
            ranges_overlap(
                ef.valid_from,
                ef.valid_until,
                new_fact.valid_from,
                new_fact.valid_until,
            )
        })
        .collect();

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
                        .execute(&mut *tx)
                        .await?;

                    let updated: Fact = sqlx::query_as::<_, Fact>(
                        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
                         valid_from, valid_until, confidence, fact_status_id, inferred, \
                         inference_depth, stale_confidence, created_at, updated_at \
                         FROM facts WHERE id = ?",
                    )
                    .bind(existing_fact.id)
                    .fetch_one(&mut *tx)
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
                    .execute(&mut *tx)
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
                        .execute(&mut *tx)
                        .await?;

                    let updated: Fact = sqlx::query_as::<_, Fact>(
                        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
                         valid_from, valid_until, confidence, fact_status_id, inferred, \
                         inference_depth, stale_confidence, created_at, updated_at \
                         FROM facts WHERE id = ?",
                    )
                    .bind(existing_fact.id)
                    .fetch_one(&mut *tx)
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
                    .execute(&mut *tx)
                    .await?;

                    facts_to_supersede.push(existing_fact.id);
                }
            }
        } else {
            // Overlap with non-explicit source → mark new fact as Disputed
            // and also mark existing overlapping facts as Disputed.
            fact_status = FactStatus::Disputed;
            for existing_fact in &overlaps {
                if existing_fact.fact_status_id != FactStatus::Disputed as i16 {
                    let old_json =
                        serde_json::json!({"fact_status_id": existing_fact.fact_status_id})
                            .to_string();
                    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
                        .bind(FactStatus::Disputed as i16)
                        .bind(now)
                        .bind(existing_fact.id)
                        .execute(&mut *tx)
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
                    .execute(&mut *tx)
                    .await?;
                }
                contradicts_pairs.push(existing_fact.id);
            }
        }
    }

    // 2. Insert the fact.
    let fact_id: i64 = sqlx::query_scalar(
        "INSERT INTO facts \
         (subject_id, predicate_id, object_id, object_literal, valid_from, valid_until, \
          confidence, fact_status_id, inferred, inference_depth, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(new_fact.subject_id)
    .bind(predicate_id)
    .bind(new_fact.object_id)
    .bind(&new_fact.object_literal)
    .bind(new_fact.valid_from)
    .bind(new_fact.valid_until)
    .bind(confidence)
    .bind(fact_status as i16)
    .bind(new_fact.inferred)
    .bind(new_fact.inference_depth)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    let fact_id = fact_id as i32;

    // 3. Resolve extraction method.
    let extraction_method_id = new_fact
        .extraction_method
        .map(|e| e as i16)
        .or_else(|| default_extraction_method(new_fact.source_type));

    // 4. Insert the source row.
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
    .execute(&mut *tx)
    .await?;

    // 5. Write created audit entry (column-only snapshot).
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
    .execute(&mut *tx)
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
        .execute(&mut *tx)
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
        .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // 8. Return the inserted fact.
    let fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(pool)
    .await?;

    Ok(fact)
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Get a fact by ID.
pub async fn get_by_id(pool: &SqlitePool, fact_id: i32) -> Result<Option<Fact>, KnowledgeError> {
    let fact: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
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
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
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
    predicate_id: i16,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
         FROM facts WHERE predicate_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(predicate_id)
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
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
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
    predicate_id: i16,
    at: DateTime<Utc>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND predicate_id = ? \
           AND fact_status_id = ? \
           AND (valid_from IS NULL OR valid_from <= ?) \
           AND (valid_until IS NULL OR valid_until > ?) \
         ORDER BY valid_from",
    )
    .bind(subject_id)
    .bind(predicate_id)
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
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
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
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
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

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;

    let old = old.ok_or(KnowledgeError::FactNotFound(fact_id))?;
    let old_json = serde_json::json!({"fact_status_id": old.fact_status_id}).to_string();

    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
        .bind(new_status as i16)
        .bind(now)
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;

    let updated: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut *tx)
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
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Determine whether two optional time ranges overlap.
///
/// A range is `[from, until)` where `None` means unbounded on that side.
fn ranges_overlap(
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
