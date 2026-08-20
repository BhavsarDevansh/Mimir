//! Fact insertion: temporal-overlap supersession, corroboration (#79),
//! provenance, and audit.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::MULTI_VALUED_PREDICATES;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::enums::RelationType;
use crate::models::fact::{Fact, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::queries::fact::status::ranges_overlap;

// ---------------------------------------------------------------------------
// Corroboration constants (#79)
// ---------------------------------------------------------------------------

/// Confidence gained per independent corroborating source.
pub(super) const CORROBORATION_BOOST: f32 = 0.05;

/// Upper bound for non-explicit fact confidence (explicit facts use 1.0).
pub(super) const NON_EXPLICIT_CONFIDENCE_CAP: f32 = 0.95;
pub(super) fn default_extraction_method(source_type: SourceType) -> Option<i16> {
    match source_type {
        SourceType::UserEdit => Some(ExtractionMethod::UserInput as i16),
        SourceType::Connector => Some(ExtractionMethod::StructuredParse as i16),
        SourceType::Inference => Some(ExtractionMethod::InferenceRule as i16),
        SourceType::Interaction => Some(ExtractionMethod::LlmExtraction as i16),
        SourceType::Import => Some(ExtractionMethod::StructuredParse as i16),
        SourceType::System => None,
    }
}

pub(super) fn changed_by_for_source_type(source_type: SourceType) -> ChangedBy {
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
pub(super) async fn fact_by_id_in_tx(
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
pub(super) fn same_object_as(
    new_object_id: Option<i32>,
    new_object_literal: Option<&str>,
    ef: &Fact,
) -> bool {
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

    // 0b. Enforce the seeded subject/object entity-type constraints for
    // entity-object facts (issue #402). Predicates without seeded constraints
    // and literal-object facts pass through; a violation rejects the fact
    // before any overlap/supersession side effects.
    crate::queries::entity::validate_predicate_in_tx(
        tx,
        relationship_type_id,
        new_fact.subject_id,
        new_fact.object_id,
    )
    .await?;

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

    let is_explicit_source = matches!(
        new_fact.source_type,
        SourceType::UserEdit | SourceType::System
    );

    if let Some(existing_fact) =
        super::corroboration::handle_corroboration(tx, new_fact, &overlaps, now).await?
    {
        return Ok(existing_fact);
    }

    let (fact_status, facts_to_supersede, contradicts_pairs) =
        super::conflict::resolve_overlap_conflict(tx, new_fact, &overlaps, is_explicit_source, now)
            .await?;

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
         (fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference, extracted_at, extraction_method_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(new_fact.source_type as i16)
    .bind(new_fact.connector_instance_id)
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
