//! Corroboration path of the fact-insert pipeline (issue #79).
//!
//! A new non-explicit fact covering the same claim as an existing
//! Active/pending fact (same object, temporally overlapping) corroborates it:
//! a source row is added to the existing fact and its confidence is boosted
//! (+0.05, capped at 0.95 for non-explicit, non-inferred facts). No new facts
//! row is created. This runs *before* the supersession path, so an explicit
//! statement still supersedes rather than corroborates. An identical
//! re-statement (non-independent source) is a no-op to avoid colliding with
//! the sources UNIQUE index.

use chrono::{DateTime, Utc};

use crate::KnowledgeError;
use crate::models::audit_log::ChangeType;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::SourceType;
use crate::queries::fact::insert::{
    CORROBORATION_BOOST, NON_EXPLICIT_CONFIDENCE_CAP, changed_by_for_source_type,
    default_extraction_method, fact_by_id_in_tx, same_object_as,
};

/// Try to corroborate an existing fact with the incoming non-explicit fact.
///
/// Returns `Ok(Some(fact))` when the claim was corroborated (or the source was
/// an exact re-statement, which is a no-op returning the existing fact), and
/// `Ok(None)` when no corroboration applies and the caller should fall through
/// to the supersession path.
pub(super) async fn handle_corroboration(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    new_fact: &NewFact,
    overlaps: &[&Fact],
    now: DateTime<Utc>,
) -> Result<Option<Fact>, KnowledgeError> {
    // A new non-explicit fact covering the same claim as an existing
    // Active/pending fact (same object, temporally overlapping) corroborates
    // it: a source row is added to the existing fact and its confidence is
    // boosted (+0.05, capped at 0.95 for non-explicit, non-inferred facts).
    // No new facts row is created. This runs *before* the supersession path
    // below, so an explicit statement still supersedes rather than
    // corroborates. An identical re-statement (non-independent source) is a
    // no-op to avoid colliding with the sources UNIQUE index.
    // Explicit sources (direct user edits and system assertions) always
    // supersede rather than corroborate. This predicate is shared with the
    // explicit-supersession path below so the two branches stay aligned.
    let is_explicit_source = matches!(
        new_fact.source_type,
        SourceType::UserEdit | SourceType::System
    );

    if is_explicit_source {
        return Ok(None);
    }

    let candidate = overlaps.iter().find(|ef| {
        if !same_object_as(new_fact.object_id, new_fact.object_literal.as_deref(), ef) {
            return false;
        }
        ef.status() == Some(FactStatus::Active) || ef.pending_confirmation
    });

    if let Some(existing_fact) = candidate {
        let connector_instance_id = new_fact.connector_instance_id;
        let raw_ref = new_fact.raw_reference.as_deref();

        // Independence check: a source with identical provenance already
        // recorded against this fact is a re-statement, not corroboration.
        let already: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM sources \
                 WHERE fact_id = ? AND source_type_id = ? \
                 AND COALESCE(connector_instance_id, 0) = COALESCE(?, 0) \
                 AND COALESCE(raw_reference, '') = COALESCE(?, '') \
                 LIMIT 1",
        )
        .bind(existing_fact.id)
        .bind(new_fact.source_type as i16)
        .bind(connector_instance_id)
        .bind(raw_ref)
        .fetch_optional(&mut **tx)
        .await?;

        if already.is_some() {
            // Duplicate re-statement: return the existing fact unchanged.
            return Ok(Some(fact_by_id_in_tx(tx, existing_fact.id).await?));
        }

        // Insert the corroborating source against the existing fact.
        let extraction_method_id = new_fact
            .extraction_method
            .map(|e| e as i16)
            .or_else(|| default_extraction_method(new_fact.source_type));
        let connector_type_id = new_fact.connector_type.map(|ct| ct as i16);

        let source_id: i64 = sqlx::query_scalar(
                "INSERT INTO sources \
                 (fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference, extracted_at, extraction_method_id) \
                 VALUES (?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id",
            )
            .bind(existing_fact.id)
            .bind(new_fact.source_type as i16)
            .bind(new_fact.connector_instance_id)
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
            "connector_instance_id": new_fact.connector_instance_id,
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
            // Keep the boost monotonic: a fact already at or above the cap
            // keeps its current confidence rather than being clamped down,
            // so corroboration never lowers confidence.
            let new_confidence = if existing_fact.confidence >= NON_EXPLICIT_CONFIDENCE_CAP {
                existing_fact.confidence
            } else {
                (existing_fact.confidence + CORROBORATION_BOOST).min(NON_EXPLICIT_CONFIDENCE_CAP)
            };
            let confidence_changed = (new_confidence - existing_fact.confidence).abs() > 1e-6;

            // A corroborated fact is current again: clear its stale flag even
            // when the confidence is unchanged (already at the cap), so new
            // provenance does not leave the row exposed as stale. The audit
            // entry and descendant cascade stay gated on an actual change.
            if confidence_changed || existing_fact.stale_confidence {
                sqlx::query(
                        "UPDATE facts SET confidence = ?, stale_confidence = FALSE, updated_at = ? WHERE id = ?",
                    )
                    .bind(new_confidence)
                    .bind(now)
                    .bind(existing_fact.id)
                    .execute(&mut **tx)
                    .await?;
            }

            if confidence_changed {
                let old_json =
                    serde_json::json!({"confidence": existing_fact.confidence}).to_string();
                let new_json = serde_json::json!({
                    "confidence": new_confidence,
                    "source_id": source_id,
                })
                .to_string();
                crate::confidence::write_confidence_change_audit(
                    tx,
                    existing_fact.id,
                    &old_json,
                    &new_json,
                    now,
                )
                .await?;

                // Cascade the confidence change to all inferred children,
                // comprehensively, within this transaction.
                crate::confidence::cascade_confidence_change_in_tx(tx, existing_fact.id, now)
                    .await?;
            }
        }

        return Ok(Some(fact_by_id_in_tx(tx, existing_fact.id).await?));
    }

    Ok(None)
}
