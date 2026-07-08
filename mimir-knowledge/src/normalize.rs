//! Shared normalize → insert boundary (Phase 3 F4 / issue #181).
//!
//! Both conversational `remember` extraction and connector ingestion funnel
//! through [`normalize_and_insert`]: a single deterministic Rust pipeline that
//! resolves entities, assigns confidence, runs the sensitivity gate, and inserts
//! facts (inheriting corroboration / supersession / inference from
//! [`crate::queries::fact::insert_fact_in_tx`]). Provenance is supplied once per
//! batch; per-fact content (including the native `raw_reference`) rides on each
//! [`NormalizedFact`].
//!
//! # Confidence
//!
//! Confidence is `confidence::initial(source_type, connector_type)` — the
//! per-source-type / per-connector reliability score. There is **no
//! extraction-method discount**: a structurally-parsed calendar fact and an
//! LLM-extracted email fact of the same source type start at the same score.
//!
//! # Sensitivity
//!
//! The same Rust `AND`-gate as conversational facts: a fact the producer flags
//! `is_sensitive` lands as `pending_confirmation` (Disputed) and surfaces via
//! `kb audit`. Rust can only narrow the flag, never widen it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::entity::{Entity, EntityType};
use crate::models::enums::{AutoCompletePolicy, ConnectorType, EventType, RecurrenceType};
use crate::models::event::NewEvent;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};

// ---------------------------------------------------------------------------
// Outcome types (shared by the extraction + connector pipelines)
// ---------------------------------------------------------------------------

/// A fact awaiting user confirmation because it was flagged as sensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingFact {
    pub fact_id: i32,
    pub subject_name: String,
    pub relationship_type: String,
    pub object_display: String,
}

/// Result of running the normalize → insert pipeline over a batch of facts.
#[derive(Debug, Default)]
pub struct ExtractionOutcome {
    pub inserted: Vec<Fact>,
    pub pending_confirmation: Vec<PendingFact>,
    pub errors: Vec<KnowledgeError>,
}

// ---------------------------------------------------------------------------
// Provenance + NormalizedFact
// ---------------------------------------------------------------------------

/// Batch-level provenance shared by every fact in one [`normalize_and_insert`]
/// call.
///
/// Carries the connector identity (when the facts come from a connector sync)
/// and the extraction method that produced them. Per-fact provenance
/// (`source_type`, `raw_reference`) lives on [`NormalizedFact`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    /// Registered connector instance backing these facts, or `None` for
    /// conversational learning. When set, `connector_type` must match the
    /// instance's registered type.
    pub connector_instance_id: Option<i32>,
    /// Denormalised connector kind. Required when `connector_instance_id` is
    /// set so the confidence model can read the reliability score without a
    /// join; `None` for conversational facts.
    pub connector_type: Option<ConnectorType>,
    /// How the facts were extracted (`LlmExtraction` for chat and LLM-driven
    /// connector extraction, `StructuredParse` for structurally-parsed
    /// connector items such as calendar events or email headers).
    pub extraction_method: ExtractionMethod,
}

impl Provenance {
    /// Provenance for conversational learning (no connector instance).
    pub const fn chat(extraction_method: ExtractionMethod) -> Self {
        Self {
            connector_instance_id: None,
            connector_type: None,
            extraction_method,
        }
    }

    /// Provenance for a connector sync. The instance id and type identify the
    /// registered connector; the extraction method describes how the raw items
    /// were turned into facts.
    pub const fn connector(
        connector_instance_id: i32,
        connector_type: ConnectorType,
        extraction_method: ExtractionMethod,
    ) -> Self {
        Self {
            connector_instance_id: Some(connector_instance_id),
            connector_type: Some(connector_type),
            extraction_method,
        }
    }
}

/// A single fact ready for the shared insert pipeline, provenance-annotated.
///
/// Both the LLM `remember` path and connector ingestion produce this type;
/// they differ only in `source_type` and (via [`Provenance`]) `extraction_method`.
/// Entity types and temporal bounds are already typed — no string parsing
/// happens inside [`normalize_and_insert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedFact {
    /// Origin family for this fact. Chat sets `UserEdit`/`Interaction` per
    /// fact (a batch may mix them); connectors set `Connector`.
    pub source_type: SourceType,
    pub subject: String,
    pub subject_type: EntityType,
    pub relationship_type: String,
    pub object: String,
    pub object_is_entity: bool,
    pub object_type: Option<EntityType>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    /// Producer's initial sensitivity flag; Rust narrows it via the
    /// sensitivity `AND`-gate.
    pub is_sensitive: bool,
    /// Whether this fact is a conversational correction. The chat adapter sets
    /// this from the LLM `Correction` classification; connectors always leave it
    /// `false` (corrections are conversational-only). When `true` the fact is
    /// routed through [`handle_correction`] even if [`correction_scope`](Self::correction_scope)
    /// is `None`, which defaults to a temporal correction at `now`.
    pub is_correction: bool,
    /// Conversational correction scope (`Some("always")` or a datetime, or
    /// `None` for a temporal correction at `now` when [`is_correction`](Self::is_correction)
    /// is set). Connectors leave this `None`.
    pub correction_scope: Option<String>,
    /// Already-parsed catalogue category IDs; validated against the DB.
    pub category_ids: Vec<i32>,
    pub recurrence: RecurrenceType,
    pub requires_user_action: bool,
    /// Native id of the source item (e.g. an email UID, a calendar event id).
    /// Required when [`Provenance::connector_instance_id`] is set.
    pub raw_reference: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-fact processing result
// ---------------------------------------------------------------------------

enum ProcessResult {
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

    for mut fact in facts {
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

        match process_normalized_fact(kg, fact, now, relationship_type_id, provenance).await {
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
    // NO extraction-method discount.
    let confidence = crate::confidence::initial(source_type, provenance.connector_type);

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
        extraction_method: Some(provenance.extraction_method),
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
        let fact = insert_sensitive_fact(kg, new_fact, now, relationship_type_id).await?;

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
    ) {
        if let Err(e) = kg.insert_event_if_absent(new_event).await {
            tracing::warn!("failed to create event overlay for fact {}: {}", fact.id, e);
        }
    }

    Ok(ProcessResult::Inserted(fact))
}

// ---------------------------------------------------------------------------
// Entity resolution
// ---------------------------------------------------------------------------

/// Resolve a name to an entity, creating it if necessary (exact match first).
///
/// The full exact → alias → FTS5 fuzzy → create chain is added by F5 (#182);
/// this boundary deliberately keeps the current resolution behaviour so chat
/// extraction stays identical.
async fn resolve_entity(
    kg: &KnowledgeGraph,
    name: &str,
    entity_type: EntityType,
) -> Result<Entity, KnowledgeError> {
    let results = queries::entity::get_by_name(kg.pool(), name).await?;
    if let Some(best) = results.into_iter().next() {
        return Ok(best.entity);
    }
    queries::entity::create_entity(kg.pool(), name, entity_type, &[]).await
}

// ---------------------------------------------------------------------------
// Event overlay derivation
// ---------------------------------------------------------------------------

/// Build an event overlay from a normalized fact, if it qualifies.
///
/// Qualification (deterministic, issue #74): the fact has a `valid_from`
/// (trigger date) AND at least one of: `valid_from` is in the future, the
/// recurrence is non-`None`, or `requires_user_action` is set.
fn event_from_extraction(
    recurrence: RecurrenceType,
    requires_user_action: bool,
    entity_id: i32,
    fact_id: i32,
    valid_from: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<NewEvent> {
    let trigger_date = valid_from?;

    let is_future = trigger_date > now;
    if !is_future && recurrence == RecurrenceType::None && !requires_user_action {
        return None;
    }

    let auto_complete_policy = if recurrence != RecurrenceType::None {
        AutoCompletePolicy::Recurring
    } else if requires_user_action {
        AutoCompletePolicy::RequiresUserAction
    } else {
        AutoCompletePolicy::AutoCompleteOnDate
    };
    let event_type = if requires_user_action {
        EventType::Task
    } else {
        EventType::Reminder
    };

    Some(NewEvent {
        fact_id,
        entity_id,
        trigger_date,
        recurrence,
        event_type,
        auto_complete_policy,
        requires_user_action,
    })
}

// ---------------------------------------------------------------------------
// Corrections (conversational-only; no-op for connectors)
// ---------------------------------------------------------------------------

/// Apply a conversational correction to the in-flight `NewFact`.
///
/// `Some("always")` — retrospective correction: the old fact was never true, so
/// every overlapping active fact is marked `Corrected`, moved to trash, and its
/// orphaned children re-evaluated. Any other string is parsed as a datetime and
/// used as the new `valid_from` (the existing insert temporal-overlap logic
/// closes the sole open-ended predecessor automatically). `None` (reached from
/// the chat adapter when the LLM emitted `Correction` with no scope) defaults to
/// a temporal correction at `now`. The handler is only called when the
/// producer flagged the fact as a correction (`is_correction`), so the `None`
/// arm is reachable and exercised by chat corrections with no scope.
async fn handle_correction(
    kg: &KnowledgeGraph,
    scope: Option<&str>,
    subject_id: i32,
    relationship_type_id: i16,
    new_fact: &mut NewFact,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    match scope {
        Some("always") => {
            let overlapping = find_active_overlapping(
                kg,
                subject_id,
                relationship_type_id,
                new_fact.valid_from,
                new_fact.valid_until,
            )
            .await?;

            let mut tx = kg.pool().begin().await?;
            let mut all_children: Vec<(i32, bool)> = Vec::new();

            for old in overlapping {
                queries::fact::set_status_tx(
                    &mut tx,
                    old.id,
                    FactStatus::Corrected,
                    now,
                    ChangedBy::System,
                )
                .await?;

                let children =
                    crate::forget::forget_fact_tx(&mut tx, old.id, ChangedBy::System, now).await?;
                all_children.extend(children);
            }

            tx.commit().await?;

            let mut seen = std::collections::HashSet::new();
            all_children.retain(|(id, _)| seen.insert(*id));

            crate::forget::evaluate_children(kg.pool(), all_children, now).await?;
        }
        Some(datetime_str) => match DateTime::parse_from_rfc3339(datetime_str) {
            Ok(dt) => {
                new_fact.valid_from = Some(dt.with_timezone::<Utc>(&Utc));
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse correction_scope datetime '{}': {}. valid_from will not be set.",
                    datetime_str,
                    e
                );
            }
        },
        None => {
            new_fact.valid_from = Some(now);
        }
    }

    Ok(())
}

/// Find active facts with the same subject + relationship_type that overlap.
async fn find_active_overlapping(
    kg: &KnowledgeGraph,
    subject_id: i32,
    relationship_type_id: i16,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let rows: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ? AND fact_status_id = ?",
    )
    .bind(subject_id)
    .bind(relationship_type_id)
    .bind(FactStatus::Active as i16)
    .fetch_all(kg.pool())
    .await?;

    Ok(rows
        .into_iter()
        .filter(|f| {
            queries::fact::ranges_overlap(f.valid_from, f.valid_until, valid_from, valid_until)
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Sensitive insertion (Disputed + pending_confirmation)
// ---------------------------------------------------------------------------

/// Insert a sensitive fact atomically with Disputed status and
/// pending_confirmation=TRUE.
async fn insert_sensitive_fact(
    kg: &KnowledgeGraph,
    new_fact: NewFact,
    now: DateTime<Utc>,
    relationship_type_id: i16,
) -> Result<Fact, KnowledgeError> {
    let confidence = new_fact
        .confidence
        .unwrap_or_else(|| crate::confidence::initial(new_fact.source_type, None));

    let mut tx = kg.pool().begin().await?;

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
