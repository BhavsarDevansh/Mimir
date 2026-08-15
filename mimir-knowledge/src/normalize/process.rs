//! Per-fact orchestration: entity resolution, confidence, sensitivity gate,
//! corrections, and insert dispatch.

use chrono::{DateTime, Utc};

use crate::models::entity::EntityType;
use crate::models::fact::{Fact, NewFact};
use crate::normalize::corrections::handle_correction;
use crate::normalize::entities::resolve_entity;
use crate::normalize::events::event_from_extraction;
use crate::normalize::overlay::{LocationOverlayApply, OverlayJob};
use crate::normalize::sensitive::insert_sensitive_fact;
use crate::normalize::types::{ExtractionOutcome, NormalizedFact, PendingFact, Provenance};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};

// Per-fact processing result
// ---------------------------------------------------------------------------

pub(super) enum ProcessResult {
    Inserted(Fact),
    Pending(PendingFact),
}

// ---------------------------------------------------------------------------
// Public entrypoint
// ---------------------------------------------------------------------------

/// Normalize and insert a batch of facts through the shared pipeline.
///
/// For each fact: canonicalise the predicate, resolve entities, assign
/// confidence from the provenance, validate categories, run the sensitivity
/// gate, and insert (inheriting corroboration / supersession / inference from
/// `insert_fact_in_tx`). Sensitive facts land as `pending_confirmation`.
/// Per-fact errors are tolerated so one bad fact never aborts the batch.
pub async fn normalize_and_insert(
    kg: &KnowledgeGraph,
    facts: Vec<NormalizedFact>,
    provenance: Provenance,
) -> Result<ExtractionOutcome, KnowledgeError> {
    let now = kg.now();
    let mut outcome = ExtractionOutcome::default();

    // Connector confidence comes from the `connector_reliability` table, not
    // the hardcoded defaults, so adjusted scores reach the pipeline (issue
    // #292). Resolved once per batch because provenance is batch-level.
    let connector_confidence: Option<f32> = match provenance.connector_type {
        Some(connector_type) => Some(kg.connector_reliability(connector_type).await?),
        None => None,
    };

    for mut fact in facts {
        // Serialise this fact's writes with the background overlay worker
        // (issue #236). Without the lock, the worker could commit a location
        // upsert between this caller's read (entity resolution / overlap
        // check) and its `insert_fact` write, staling the deferred WAL
        // transaction with an immediate, un-retriable `SQLITE_BUSY`. The
        // guard is per-fact so the worker can drain overlays between facts
        // and reads stay concurrent.
        let _write_guard = kg.write_lock().lock().await;

        // Canonicalise the predicate: `ensure_relationship_type` normalises,
        // consults the alias table (single source of truth), and auto-creates
        // a canonical type + self-alias on a miss. The id threads through to
        // the per-fact processor so the resolution is not repeated downstream.
        let relationship_type_id = match kg.ensure_relationship_type(&fact.relationship_type).await
        {
            Ok(id) => id,
            Err(error) => {
                outcome.errors.push(error);
                continue;
            }
        };

        // `ensure_relationship_type` always creates/resolves the type, so the
        // name lookup succeeds in practice; fall back to the normalized input
        // purely as a defensive measure.
        let canonical_name = kg.relationship_type_name(relationship_type_id).await;
        fact.relationship_type = canonical_name
            .unwrap_or_else(|| crate::normalize_alias(&fact.relationship_type).unwrap_or_default());

        match process_normalized_fact(
            kg,
            fact,
            now,
            relationship_type_id,
            provenance,
            connector_confidence,
        )
        .await
        {
            Ok(ProcessResult::Inserted(f)) => outcome.inserted.push(f),
            Ok(ProcessResult::Pending(p)) => outcome.pending_confirmation.push(p),
            Err(error) => outcome.errors.push(error),
        }
    }

    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Per-fact orchestration (resolve -> confidence -> sensitivity-gate -> insert)
// ---------------------------------------------------------------------------

async fn process_normalized_fact(
    kg: &KnowledgeGraph,
    extracted: NormalizedFact,
    now: DateTime<Utc>,
    relationship_type_id: i16,
    provenance: Provenance,
    connector_confidence: Option<f32>,
) -> Result<ProcessResult, KnowledgeError> {
    // The original subject string is captured before `resolve_entity` shadows it
    // with the resolved `Entity`, so the pending-confirmation result and the
    // unknown-category warning can report the user-facing input verbatim.
    // `relationship_type` and `object` stay owned (only borrowed below), so
    // they are cloned lazily only on the pending path.
    let subject_name = extracted.subject.clone();

    let NormalizedFact {
        source_type,
        subject,
        subject_type,
        relationship_type,
        object,
        object_is_entity,
        object_type,
        valid_from,
        valid_until,
        is_sensitive,
        is_correction,
        ref correction_scope,
        ref category_ids,
        recurrence,
        requires_user_action,
        ref raw_reference,
        extraction_method,
        event_type,
        location,
    } = extracted;

    // Connector provenance requires a native raw_reference (the insert gate
    // also enforces extraction_method, which the Provenance always supplies).
    if provenance.connector_instance_id.is_some() && raw_reference.is_none() {
        return Err(KnowledgeError::Validation(
            "Connector provenance requires connector_instance_id, raw_reference, and extraction_method"
                .to_string(),
        ));
    }

    // Resolve entities.
    let subject = resolve_entity(kg, &subject, subject_type).await?;
    let (object_id, object_literal) = if object_is_entity {
        let ot = object_type.unwrap_or(EntityType::Concept);
        let obj = resolve_entity(kg, &object, ot).await?;
        (Some(obj.id), None)
    } else {
        (None, Some(object.clone()))
    };

    // If this fact establishes a preferred name, register the object as an alias
    // so future lookups by that short name resolve to the canonical entity.
    if relationship_type == "preferred_name" {
        let alias = &object;
        if let Err(e) = kg.add_alias(subject.id, alias).await {
            tracing::warn!(
                "Failed to add preferred-name alias '{}' to entity {}: {}",
                alias,
                subject.id,
                e
            );
        }

        // If a bare-name duplicate entity exists (created before the alias was
        // wired up), auto-merge it when it looks accidental (very few facts).
        if let Ok(candidates) = queries::entity::get_by_name(kg.pool(), alias).await {
            for cand in candidates {
                if cand.entity.id == subject.id {
                    continue;
                }
                if cand.entity.name.to_lowercase() == alias.to_lowercase() {
                    match sqlx::query_as::<_, (i64,)>(
                        "SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?",
                    )
                    .bind(cand.entity.id)
                    .bind(cand.entity.id)
                    .fetch_one(kg.pool())
                    .await
                    {
                        Ok((fact_count,)) if fact_count <= 2 => {
                            if let Err(e) = queries::entity::auto_merge_pair(
                                kg.pool(),
                                subject.id,
                                cand.entity.id,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "Failed to auto-merge duplicate entity {} into {}: {}",
                                    cand.entity.id,
                                    subject.id,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to count facts for candidate entity {} during auto-merge check: {}",
                                cand.entity.id,
                                e
                            );
                        }
                        _ => {}
                    }
                    break;
                }
            }
        }
    }

    // Confidence: reliability score for the source type / connector kind, with
    // NO extraction-method discount. Connector facts use the
    // `connector_reliability` table score resolved once per batch (issue
    // #292); other source types keep the `confidence::initial` defaults.
    let confidence =
        crate::confidence::resolve_initial_confidence(None, source_type, connector_confidence);

    // Validate and collect category IDs.
    let mut valid_category_ids = Vec::new();
    for &cat_id in category_ids {
        match kg.get_category(cat_id).await? {
            Some(_) => valid_category_ids.push(cat_id),
            None => {
                tracing::warn!(
                    "unknown category {} for fact '{} {} {}'; ignoring",
                    cat_id,
                    subject_name,
                    relationship_type,
                    object,
                );
            }
        }
    }

    let mut new_fact = NewFact {
        subject_id: subject.id,
        relationship_type: relationship_type.clone(),
        object_id,
        object_literal,
        valid_from,
        valid_until,
        source_type,
        connector_instance_id: provenance.connector_instance_id,
        connector_type: provenance.connector_type,
        raw_reference: raw_reference.clone(),
        extraction_method: Some(extraction_method.unwrap_or(provenance.extraction_method)),
        inferred: false,
        inference_depth: 0,
        confidence: Some(confidence),
        parent_fact_ids: Vec::new(),
        category_ids: valid_category_ids,
    };

    // Corrections are conversational-only: the chat adapter sets
    // `is_correction` from the LLM `Correction` classification, while
    // connectors always leave it `false`. Gate on the correction *signal*
    // (not merely a present scope) so a conversational `Correction` with no
    // scope still defaults to a temporal correction at `now`.
    if is_correction {
        handle_correction(
            kg,
            correction_scope.as_deref(),
            subject.id,
            relationship_type_id,
            &mut new_fact,
            now,
        )
        .await?;
    }

    // Sensitivity gate (#142): the producer provides an initial is_sensitive
    // flag, but Rust validates it against the fact's catalogue categories and
    // object text. Rust can only narrow (AND gate) — it never flags a fact as
    // sensitive when the producer did not.
    if crate::sensitivity::is_sensitive(is_sensitive, &new_fact.category_ids, &object) {
        // The location-overlay shape is persisted inside the same transaction
        // as the pending fact (issue #226), so a confirmable fact can never
        // exist without the shape `confirm_fact` needs to rebuild its
        // `entity_locations` row.
        let fact =
            insert_sensitive_fact(kg, new_fact, now, relationship_type_id, location.as_ref())
                .await?;

        // Only add to in-memory cache after successful commit.
        kg.pending_confirmations().write().await.insert(fact.id);

        // Persist the derived event shape so `confirm_fact` can rebuild the
        // overlay from the extracted recurrence/event_type/policy/
        // requires_user_action instead of synthesising one-time defaults.
        if let Some(new_event) = event_from_extraction(
            recurrence,
            requires_user_action,
            subject.id,
            fact.id,
            valid_from,
            now,
            event_type,
        ) {
            if let Err(e) =
                queries::event::insert_pending_event_meta(kg.pool(), fact.id, &new_event).await
            {
                tracing::warn!(
                    "failed to persist pending event meta for fact {}: {}",
                    fact.id,
                    e
                );
            }
        }

        return Ok(ProcessResult::Pending(PendingFact {
            fact_id: fact.id,
            subject_name,
            relationship_type: relationship_type.clone(),
            object_display: object.clone(),
        }));
    }

    // Non-sensitive facts go through the normal path.
    let fact = kg.insert_fact(new_fact).await?;

    // Events subsystem (#74): create a lifecycle overlay when the fact is
    // time-bound (future date), recurring, or requires user action.
    if let Some(new_event) = event_from_extraction(
        recurrence,
        requires_user_action,
        subject.id,
        fact.id,
        valid_from,
        now,
        event_type,
    ) {
        if let Err(e) = kg.insert_event_if_absent(new_event).await {
            tracing::warn!("failed to create event overlay for fact {}: {}", fact.id, e);
        }
    }

    // Entity-locations overlay (Phase 3 S3 / #193): a "where" fact carries a
    // typed location that is geocoded (filling the missing half) and upserted
    // for the subject entity with the *inserted fact's* temporal bounds. The
    // bounds are read from `fact` (not the pre-correction `valid_from`/
    // `valid_until` bindings) because `handle_correction` may have mutated
    // `new_fact.valid_from` before the insert (a correction scope of `None`
    // becomes `now`, a datetime scope becomes that datetime); using the
    // original bindings would make the `entity_locations` row diverge from its
    // source fact and skip prior-location supersession.
    //
    // The geocode + upsert is offloaded to a single background worker (see
    // [`OverlayJob`]) so a connector batch emitting many location facts is not
    // gated on the geocoder's rate limit (~1 req/sec for Nominatim). The job
    // carries a clone of the geocoder read at submit time and is processed in
    // submission order, preserving move/supersession ordering across batches.
    if let Some(loc) = location {
        // When the fact's object is a Place entity (e.g. a `took_photo_at
        // <place>` connector fact, Phase 3 C2 / #196), anchor the place's own
        // coordinates alongside the subject's Visited row.
        let place_anchor = if object_is_entity && object_type == Some(EntityType::Place) {
            object_id
        } else {
            None
        };
        let _ = kg
            .location_overlay_tx()
            .send(OverlayJob::Apply(LocationOverlayApply {
                geocoder: kg.geocoder().cloned(),
                entity_id: subject.id,
                location: loc,
                valid_from: fact.valid_from,
                valid_until: fact.valid_until,
                fact_id: fact.id,
                place_anchor,
            }));
    }

    Ok(ProcessResult::Inserted(fact))
}
