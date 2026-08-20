//! Sensitive-fact insertion: Disputed status + pending_confirmation + audit.

use chrono::{DateTime, Utc};

use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::normalize::types::NormalizedLocation;
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};
// ---------------------------------------------------------------------------

/// Insert a sensitive fact atomically with Disputed status and
/// pending_confirmation=TRUE, persisting the location-overlay shape in the
/// same transaction when the fact carries one (issue #226).
pub(super) async fn insert_sensitive_fact(
    kg: &KnowledgeGraph,
    new_fact: NewFact,
    now: DateTime<Utc>,
    relationship_type_id: i16,
    location: Option<&NormalizedLocation>,
) -> Result<Fact, KnowledgeError> {
    let confidence = new_fact
        .confidence
        .unwrap_or_else(|| crate::confidence::initial(new_fact.source_type, None));

    let mut tx = kg.pool().begin().await?;

    // Enforce the seeded subject/object entity-type constraints (issue #402)
    // before writing the pending row, mirroring `insert_fact_in_tx`.
    queries::entity::validate_predicate_in_tx(
        &mut tx,
        relationship_type_id,
        new_fact.subject_id,
        new_fact.object_id,
    )
    .await?;

    let memory_priority_id: i16 = sqlx::query_scalar(
        "SELECT COALESCE(r.default_memory_priority_id, p.id) \
         FROM relationship_types r \
         CROSS JOIN memory_priorities p \
         WHERE r.id = ? AND p.name = 'Normal'",
    )
    .bind(relationship_type_id)
    .fetch_one(&mut *tx)
    .await?;

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
    .bind(FactStatus::Disputed as i16)
    .bind(new_fact.inferred)
    .bind(new_fact.inference_depth)
    .bind(true)
    .bind(memory_priority_id)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    let fact_id = fact_id as i32;

    let extraction_method_id = new_fact.extraction_method.map(|e| e as i16);
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
    .execute(&mut *tx)
    .await?;

    let new_value = serde_json::json!({
        "fact_id": fact_id,
        "confidence": confidence,
        "fact_status_id": FactStatus::Disputed as i16,
        "pending_confirmation": true,
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
    .bind(ChangedBy::System as i16)
    .bind(Some("Sensitive fact pending confirmation"))
    .execute(&mut *tx)
    .await?;

    // Persist catalogue category links within the same transaction, mirroring
    // the normal insert path (`insert_fact_internal` / `insert_facts_batch`).
    // Without this, sensitive facts lost their categories, breaking
    // category-based reads and downstream memory/sensitivity logic.
    for category_id in &new_fact.category_ids {
        sqlx::query("INSERT OR IGNORE INTO fact_categories (fact_id, category_id) VALUES (?, ?)")
            .bind(fact_id)
            .bind(category_id)
            .execute(&mut *tx)
            .await?;
    }

    // Persist the location-overlay shape in the same transaction as the fact
    // (issue #226): a confirmable fact must never exist without the shape
    // `confirm_fact` needs to rebuild its `entity_locations` row. If either
    // write fails, both roll back and the caller reports an error instead of
    // leaving a confirmable fact that would lose its location payload.
    if let Some(loc) = location {
        queries::location::insert_pending_location_meta_in_tx(
            &mut tx,
            fact_id,
            loc.location_type as i16,
            loc.address.as_deref(),
            loc.latitude,
            loc.longitude,
            loc.timezone.as_deref(),
        )
        .await?;
    }

    tx.commit().await?;

    let fact: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(kg.pool())
    .await?;

    Ok(fact)
}
