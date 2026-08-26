//! Sensitive-fact confirmation / rejection lifecycle (issue #141).

use crate::inference::CascadeContext;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::enums::{AutoCompletePolicy, EventType, LocationType, RecurrenceType};
use crate::models::event::NewEvent;
use crate::models::fact::{Fact, FactStatus};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::normalize::{LocationOverlayApply, NormalizedLocation, apply_location_overlay};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};

/// Confirm a pending sensitive fact: flip to Active with confidence 1.0.
pub async fn confirm_fact(kg: &KnowledgeGraph, fact_id: i32) -> Result<Fact, KnowledgeError> {
    let now = kg.now();

    let mut tx = kg.pool().begin().await?;

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

    if !old.pending_confirmation {
        return Err(KnowledgeError::Validation(
            "Fact is not awaiting confirmation.".to_string(),
        ));
    }

    let old_json = serde_json::json!({
        "fact_status_id": old.fact_status_id,
        "confidence": old.confidence,
        "pending_confirmation": true,
    })
    .to_string();

    sqlx::query(
        "UPDATE facts SET fact_status_id = ?, confidence = ?, pending_confirmation = FALSE, updated_at = ? WHERE id = ?",
    )
    .bind(FactStatus::Active as i16)
    .bind(1.0f32)
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

    let new_json = serde_json::json!({
        "fact_status_id": updated.fact_status_id,
        "confidence": updated.confidence,
        "pending_confirmation": false,
    })
    .to_string();

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
    .bind(ChangedBy::User as i16)
    .bind(Some("User confirmed sensitive fact"))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    kg.pending_confirmations().write().await.remove(&fact_id);

    // A confirmed fact is a fact mutation that can re-rank condensed memory
    // (status and confidence changed), so route it through the same dirty
    // signal as insert/update/forget/restore: the `memory.condensation` hook
    // rebuilds on demand (issue #386).
    kg.set_condensation_dirty();

    // Events subsystem (#74): sensitive facts skip overlay creation at
    // extraction time (they return `Pending` before reaching the event block).
    // Now that the fact is confirmed and Active, rebuild the overlay from the
    // event shape persisted at extraction time (`pending_event_meta`) so the
    // extracted recurrence/event_type/auto_complete_policy/requires_user_action
    // survive the sensitivity gate. Legacy pending facts that predate the
    // `pending_event_meta` table fall back to a one-time `Reminder` overlay for
    // future-dated facts. The insert is idempotent, so re-confirmation is safe.
    if let Some(valid_from) = updated.valid_from {
        // The fact is already committed as confirmed at this point, so overlay
        // rebuild must never propagate errors to the caller — a failure here
        // would make `confirm_fact` appear to fail after the fact is no longer
        // pending. Log and fall back to the legacy one-time overlay path.
        let meta = match queries::event::get_pending_event_meta(kg.pool(), updated.id).await {
            Ok(meta) => meta,
            Err(e) => {
                tracing::warn!(
                    "failed to read pending event meta for confirmed fact {}: {};                      falling back to one-time overlay",
                    updated.id,
                    e
                );
                None
            }
        };
        match meta {
            Some(meta) => {
                let new_event = NewEvent {
                    fact_id: updated.id,
                    entity_id: updated.subject_id,
                    trigger_date: valid_from,
                    recurrence: RecurrenceType::try_from(meta.recurrence_type_id)
                        .unwrap_or(RecurrenceType::None),
                    recurrence_rule: meta.recurrence_rule,
                    recurrence_interval: meta.recurrence_interval,
                    recurrence_until: meta.recurrence_until,
                    event_type: EventType::try_from(meta.event_type_id)
                        .unwrap_or(EventType::Reminder),
                    auto_complete_policy: AutoCompletePolicy::try_from(
                        meta.auto_complete_policy_id,
                    )
                    .unwrap_or(AutoCompletePolicy::AutoCompleteOnDate),
                    requires_user_action: meta.requires_user_action,
                };
                if let Err(e) = kg.insert_event_if_absent(new_event).await {
                    tracing::warn!(
                        "failed to create event overlay for confirmed fact {}: {}",
                        updated.id,
                        e
                    );
                }
                // Meta is consumed; drop it so it cannot drift from the overlay.
                if let Err(e) =
                    queries::event::delete_pending_event_meta(kg.pool(), updated.id).await
                {
                    tracing::warn!(
                        "failed to clear pending event meta for fact {}: {}",
                        updated.id,
                        e
                    );
                }
            }
            None => {
                // Legacy pending fact (predates pending_event_meta): derive a
                // one-time Reminder overlay for future-dated facts only.
                if valid_from > now {
                    let new_event = NewEvent {
                        fact_id: updated.id,
                        entity_id: updated.subject_id,
                        trigger_date: valid_from,
                        recurrence: RecurrenceType::None,
                        recurrence_rule: None,
                        recurrence_interval: 1,
                        recurrence_until: None,
                        event_type: EventType::Reminder,
                        auto_complete_policy: AutoCompletePolicy::AutoCompleteOnDate,
                        requires_user_action: false,
                    };
                    if let Err(e) = kg.insert_event_if_absent(new_event).await {
                        tracing::warn!(
                            "failed to create event overlay for confirmed fact {}: {}",
                            updated.id,
                            e
                        );
                    }
                }
            }
        }
    }

    // Entity-locations overlay (issue #226): sensitive "where" facts return
    // `Pending` before the location-overlay block in `process_normalized_fact`,
    // so the structured geo data would otherwise be lost across the
    // confirmation boundary. Now that the fact is confirmed and Active,
    // rebuild the `entity_locations` row from the location shape persisted at
    // extraction time (`pending_location_meta`): re-run the geocode-fill
    // (address -> coords or coords -> address) and upsert with the *confirmed
    // fact's* temporal bounds and id, exactly as the non-sensitive path does.
    // Legacy pending facts that predate the `pending_location_meta` table have
    // no shape and get no overlay. The fact is already committed as confirmed,
    // so a rebuild failure must never propagate to the caller — log and
    // continue (the row is still created without the geocoded half, matching
    // the non-sensitive path's geocoder-error tolerance).
    match queries::location::get_pending_location_meta(kg.pool(), updated.id).await {
        Ok(Some(meta)) => {
            let apply = LocationOverlayApply {
                geocoder: kg.geocoder().cloned(),
                entity_id: updated.subject_id,
                location: NormalizedLocation {
                    location_type: LocationType::try_from(meta.location_type_id)
                        .unwrap_or(LocationType::Visited),
                    address: meta.address,
                    latitude: meta.latitude,
                    longitude: meta.longitude,
                    timezone: meta.timezone,
                },
                valid_from: updated.valid_from,
                valid_until: updated.valid_until,
                fact_id: updated.id,
                // Place anchoring (Phase 3 C2) is not rebuilt on this path:
                // `pending_location_meta` stores the NormalizedLocation shape
                // only, so a sensitive Place-object fact gets the subject's
                // location row but no `Geographic` anchor for the place.
                place_anchor: None,
            };
            let overlay_ok = apply_location_overlay(kg.pool(), kg.write_lock(), apply).await;
            if overlay_ok {
                // Meta is consumed; drop it so it cannot drift from the overlay.
                if let Err(e) =
                    queries::location::delete_pending_location_meta(kg.pool(), updated.id).await
                {
                    tracing::warn!(
                        "failed to clear pending location meta for fact {}: {}",
                        updated.id,
                        e
                    );
                }
            } else {
                // The overlay write failed; retain the meta so a retry can
                // rebuild the row instead of losing the only location payload.
                tracing::warn!(
                    "location overlay rebuild failed for confirmed fact {}; retaining pending_location_meta for retry",
                    updated.id
                );
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            "failed to read pending location meta for confirmed fact {}: {}",
            updated.id,
            e
        ),
    }

    // Trigger inference now that the fact is Active and cascade inferred facts.
    let mut ctx = CascadeContext::new();
    match kg
        .rule_engine()
        .evaluate_insert(&updated, kg, &mut ctx)
        .await
    {
        Ok(inferred) => {
            for mut inferred_fact in inferred {
                inferred_fact.inferred = true;
                inferred_fact.source_type = SourceType::Inference;
                inferred_fact.extraction_method = Some(ExtractionMethod::InferenceRule);
                if let Err(e) = kg.insert_fact_internal(inferred_fact, &mut ctx).await {
                    tracing::warn!("inference cascade failed: {}", e);
                }
            }
        }
        Err(e) => {
            tracing::warn!("inference evaluation failed: {}", e);
        }
    }

    Ok(updated)
}

/// Reject a pending sensitive fact: hard-delete with audit trail.
///
/// `reason`, if supplied, overrides the default audit message
/// ("User rejected sensitive fact").
pub async fn reject_fact(
    kg: &KnowledgeGraph,
    fact_id: i32,
    reason: Option<&str>,
) -> Result<(), KnowledgeError> {
    let audit_reason = match reason {
        Some(r) => format!("User rejected sensitive fact: {r}"),
        None => "User rejected sensitive fact".to_string(),
    };
    let now = kg.now();

    let mut tx = kg.pool().begin().await?;

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

    if !old.pending_confirmation {
        return Err(KnowledgeError::Validation(
            "Fact is not awaiting confirmation.".to_string(),
        ));
    }

    let old_json = serde_json::to_string(&old)
        .map_err(|e| KnowledgeError::Validation(format!("JSON serialization failed: {}", e)))?;

    // Write rejection audit entry before deletion.
    sqlx::query(
        "INSERT INTO fact_audit_log \
         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind(ChangeType::Rejected as i16)
    .bind(old_json)
    .bind(None::<&str>)
    .bind(now)
    .bind(ChangedBy::User as i16)
    .bind(Some(audit_reason))
    .execute(&mut *tx)
    .await?;

    // Clear dependency edges first: `fact_dependencies` uses ON DELETE
    // RESTRICT (migration 017), so a pending fact with edges can only be
    // hard-deleted once those rows are removed.
    sqlx::query("DELETE FROM fact_dependencies WHERE parent_fact_id = ? OR child_fact_id = ?")
        .bind(fact_id)
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;

    // Hard-delete the fact. Sources cascade; audit rows persist.
    sqlx::query("DELETE FROM facts WHERE id = ?")
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    kg.pending_confirmations().write().await.remove(&fact_id);

    Ok(())
}

#[cfg(test)]
#[path = "confirm_tests.rs"]
mod confirm_tests;
