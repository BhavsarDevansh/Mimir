//! Overlap-conflict resolution for the fact-insert pipeline.
//!
//! Explicit sources (direct user edits and system assertions) supersede every
//! overlapping fact (closing the sole open-ended predecessor temporally and
//! wiring `Supersedes` edges); non-explicit overlaps mark both sides Disputed
//! and wire `Contradicts` edges. Pure decision + write helper so the insert
//! orchestrator stays readable.

use chrono::{DateTime, Utc};

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::fact::{Fact, FactStatus, NewFact};

/// Resolve overlapping facts for a new fact and persist the status changes.
///
/// Returns `(status for the new fact, ids superseded, ids contradicted)`.
pub(super) async fn resolve_overlap_conflict(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    new_fact: &NewFact,
    overlaps: &[Fact],
    is_explicit_source: bool,
    now: DateTime<Utc>,
) -> Result<(FactStatus, Vec<i32>, Vec<i32>), KnowledgeError> {
    let mut fact_status = FactStatus::Active;
    let mut facts_to_supersede: Vec<i32> = Vec::new();
    let mut contradicts_pairs: Vec<i32> = Vec::new();

    if !overlaps.is_empty() {
        if is_explicit_source {
            // Explicit replacement: supersede all overlapping facts.
            for existing_fact in overlaps {
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
                    // Shared status transition (audit + overlay retirement on
                    // Superseded, issue #413) so the insert pipeline and the
                    // other supersession paths cannot drift apart.
                    super::status::set_status_tx(
                        tx,
                        existing_fact.id,
                        FactStatus::Superseded,
                        now,
                        ChangedBy::System,
                    )
                    .await?;
                    facts_to_supersede.push(existing_fact.id);
                }
            }
        } else {
            // Overlap with non-explicit source → mark new fact as Disputed
            // and also mark existing overlapping facts as Disputed.
            fact_status = FactStatus::Disputed;
            for existing_fact in overlaps {
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

    Ok((fact_status, facts_to_supersede, contradicts_pairs))
}
