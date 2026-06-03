//! Fact extraction pipeline: LLM → Rust validation → entity resolution →
//! confidence assignment → sensitive confirmation → fact insertion.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use mimir_core::llm::backend::LlmBackend;
use mimir_core::llm::types::Message;

use crate::inference::CascadeContext;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::entity::{Entity, EntityType};
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};

// ---------------------------------------------------------------------------
// Extraction schema
// ---------------------------------------------------------------------------

/// Classification returned by the LLM for each extracted fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum Classification {
    Explicit,
    Casual,
    Correction,
}

/// Temporal bounds for a fact, optional.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Temporal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
}

/// A single fact extracted by the LLM via the `remember` tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtractedFact {
    pub classification: Classification,
    pub subject: String,
    pub subject_type: String,
    pub predicate: String,
    pub object: String,
    pub object_is_entity: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal: Option<Temporal>,
    #[serde(default)]
    pub is_sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_scope: Option<String>,
}

/// Wrapper returned by the `remember` tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RememberOutput {
    pub facts: Vec<ExtractedFact>,
}

// ---------------------------------------------------------------------------
// Outcome types
// ---------------------------------------------------------------------------

/// A fact awaiting user confirmation because it was flagged as sensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingFact {
    pub fact_id: i32,
    pub subject_name: String,
    pub predicate: String,
    pub object_display: String,
}

/// Result of running the extraction pipeline over a user message.
#[derive(Debug, Default)]
pub struct ExtractionOutcome {
    pub inserted: Vec<Fact>,
    pub pending_confirmation: Vec<PendingFact>,
    pub corroborated: Vec<i32>,
    pub errors: Vec<KnowledgeError>,
}

// ---------------------------------------------------------------------------
// Tool schema
// ---------------------------------------------------------------------------

/// Build the JSON Schema for the `remember` tool.
fn remember_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "remember",
            "description": "Extract structured facts from user messages. Each fact is a subject-predicate-object triple with classification, temporal bounds, and sensitivity flags.",
            "parameters": {
                "type": "object",
                "properties": {
                    "facts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "classification": {
                                    "type": "string",
                                    "enum": ["Explicit", "Casual", "Correction"],
                                    "description": "How the fact was stated. Explicit = direct assertion. Casual = passing mention. Correction = user is correcting a previous fact."
                                },
                                "subject": {
                                    "type": "string",
                                    "description": "The entity the fact is about."
                                },
                                "subject_type": {
                                    "type": "string",
                                    "enum": ["Person", "Place", "Event", "Object", "Concept", "Organization", "Activity", "DateTime"],
                                    "description": "Entity type of the subject."
                                },
                                "predicate": {
                                    "type": "string",
                                    "description": "The relationship or property being asserted."
                                },
                                "object": {
                                    "type": "string",
                                    "description": "The value or target of the predicate."
                                },
                                "object_is_entity": {
                                    "type": "boolean",
                                    "description": "Whether the object is an entity (true) or a literal string (false)."
                                },
                                "object_type": {
                                    "type": "string",
                                    "enum": ["Person", "Place", "Event", "Object", "Concept", "Organization", "Activity", "DateTime"],
                                    "description": "Entity type of the object, if object_is_entity is true."
                                },
                                "temporal": {
                                    "type": "object",
                                    "properties": {
                                        "valid_from": {
                                            "type": "string",
                                            "description": "ISO-8601 datetime when this fact became true."
                                        },
                                        "valid_until": {
                                            "type": "string",
                                            "description": "ISO-8601 datetime when this fact ceased being true."
                                        }
                                    }
                                },
                                "is_sensitive": {
                                    "type": "boolean",
                                    "description": "Whether this fact involves health, financial, relationship, or other sensitive topics."
                                },
                                "correction_scope": {
                                    "type": "string",
                                    "description": "For Corrections only: an ISO-8601 datetime (when the new truth began) or the literal string 'always' (the old fact was never true)."
                                }
                            },
                            "required": ["classification", "subject", "subject_type", "predicate", "object", "object_is_entity"]
                        }
                    }
                },
                "required": ["facts"]
            }
        }
    })
}

/// Build the system prompt for fact extraction.
fn extraction_prompt() -> String {
    String::from(
        "You are a fact extractor. Your job is to read the user's message and extract \
any facts about the user into structured triples (subject-predicate-object).\n\n\
Rules:\n\
- Classify each fact as Explicit (direct assertion), Casual (passing mention), or Correction.\n\
- For Corrections, set correction_scope to either an ISO-8601 datetime or the string 'always'.\n\
- Mark health, financial, relationship, religious, political, or legal facts as is_sensitive.\n\
- Subject and object types must be one of: Person, Place, Event, Object, Concept, Organization, Activity, DateTime.\n\
- Output ONLY via the 'remember' tool. Do not output free text.",
    )
}

// ---------------------------------------------------------------------------
// Classification → SourceType + confidence
// ---------------------------------------------------------------------------

fn source_type_for(classification: Classification) -> SourceType {
    match classification {
        Classification::Explicit | Classification::Correction => SourceType::UserEdit,
        Classification::Casual => SourceType::Interaction,
    }
}

fn confidence_for(classification: Classification) -> f32 {
    crate::confidence::initial(source_type_for(classification), None)
}

// ---------------------------------------------------------------------------
// Entity resolution
// ---------------------------------------------------------------------------

/// Resolve a name to an entity ID, creating the entity if necessary.
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

/// Parse an entity type string into the Rust enum.
fn parse_entity_type(s: &str) -> Result<EntityType, KnowledgeError> {
    match s {
        "Person" => Ok(EntityType::Person),
        "Place" => Ok(EntityType::Place),
        "Event" => Ok(EntityType::Event),
        "Object" => Ok(EntityType::Object),
        "Concept" => Ok(EntityType::Concept),
        "Organization" => Ok(EntityType::Organization),
        "Activity" => Ok(EntityType::Activity),
        "DateTime" => Ok(EntityType::DateTime),
        _ => Err(KnowledgeError::Validation(format!(
            "Invalid entity_type: {}",
            s
        ))),
    }
}

// ---------------------------------------------------------------------------
// Dedup check (stub for #79)
// ---------------------------------------------------------------------------

/// Check whether an identical active fact already exists.
/// Returns the existing fact ID if found.
async fn find_existing_fact(
    kg: &KnowledgeGraph,
    subject_id: i32,
    predicate_id: i16,
    object_id: Option<i32>,
    object_literal: Option<&str>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> Result<Option<i32>, KnowledgeError> {
    let rows: Vec<(i32,)> = sqlx::query_as(
        "SELECT id FROM facts \
         WHERE subject_id = ? AND predicate_id = ? \
         AND object_id IS ? AND object_literal IS ? \
         AND (fact_status_id = ? OR pending_confirmation = TRUE) \
         AND ((valid_from IS ?) OR (valid_from = ?)) \
         AND ((valid_until IS ?) OR (valid_until = ?))",
    )
    .bind(subject_id)
    .bind(predicate_id)
    .bind(object_id)
    .bind(object_literal)
    .bind(FactStatus::Active as i16)
    .bind(valid_from)
    .bind(valid_from)
    .bind(valid_until)
    .bind(valid_until)
    .fetch_all(kg.pool())
    .await?;

    Ok(rows.into_iter().next().map(|r| r.0))
}

// ---------------------------------------------------------------------------
// Correction helpers
// ---------------------------------------------------------------------------

/// Find active facts with the same subject + predicate that overlap the new fact.
async fn find_active_overlapping(
    kg: &KnowledgeGraph,
    subject_id: i32,
    predicate_id: i16,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let rows: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND predicate_id = ? AND fact_status_id = ?",
    )
    .bind(subject_id)
    .bind(predicate_id)
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
// Pipeline entrypoint
// ---------------------------------------------------------------------------

/// Run the fact extraction pipeline on a single user message.
///
/// 1. Calls the LLM via the `remember` tool.
/// 2. Validates schema, resolves entities, checks dedup.
/// 3. Assigns confidence based on classification.
/// 4. Handles corrections (temporal or retrospective).
/// 5. Flags sensitive facts for confirmation.
/// 6. Inserts facts, attaches sources, triggers inference.
pub async fn extract_facts(
    kg: &KnowledgeGraph,
    llm: &Arc<dyn LlmBackend>,
    user_message: &str,
) -> Result<ExtractionOutcome, KnowledgeError> {
    let now = kg.now();

    // 1. Call LLM.
    let messages = vec![
        Message::system(extraction_prompt()),
        Message::user(user_message),
    ];
    let tool = remember_tool_schema();

    let (assistant_msg, _usage) = llm
        .chat_message(messages, Some(vec![tool]))
        .await
        .map_err(|e| KnowledgeError::Validation(format!("LLM call failed: {}", e)))?;

    // 2. Parse tool calls.
    let tool_calls = assistant_msg
        .tool_calls
        .ok_or_else(|| KnowledgeError::Validation("LLM did not emit a tool call.".to_string()))?;

    let first_call = tool_calls
        .into_iter()
        .next()
        .ok_or_else(|| KnowledgeError::Validation("LLM tool call list was empty.".to_string()))?;

    let extracted: RememberOutput =
        serde_json::from_str(&first_call.function.arguments).map_err(|e| {
            KnowledgeError::Validation(format!("Failed to parse tool arguments: {}", e))
        })?;

    let mut outcome = ExtractionOutcome::default();

    for fact in extracted.facts {
        match process_extracted_fact(kg, fact, now).await {
            Ok(result) => match result {
                ProcessResult::Inserted(f) => outcome.inserted.push(f),
                ProcessResult::Pending(p) => outcome.pending_confirmation.push(p),
                ProcessResult::Corroborated(id) => outcome.corroborated.push(id),
            },
            Err(e) => outcome.errors.push(e),
        }
    }

    Ok(outcome)
}

enum ProcessResult {
    Inserted(Fact),
    Pending(PendingFact),
    Corroborated(i32),
}

async fn process_extracted_fact(
    kg: &KnowledgeGraph,
    extracted: ExtractedFact,
    now: DateTime<Utc>,
) -> Result<ProcessResult, KnowledgeError> {
    // 3. Validate entity types.
    let subject_type = parse_entity_type(&extracted.subject_type)?;
    let object_type = extracted
        .object_type
        .as_ref()
        .map(|s| parse_entity_type(s))
        .transpose()?;

    // 4. Parse temporal.
    let valid_from = extracted
        .temporal
        .as_ref()
        .and_then(|t| t.valid_from.as_ref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone::<Utc>(&Utc));

    let valid_until = extracted
        .temporal
        .as_ref()
        .and_then(|t| t.valid_until.as_ref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone::<Utc>(&Utc));

    // 5. Resolve entities.
    let subject = resolve_entity(kg, &extracted.subject, subject_type).await?;
    let (object_id, object_literal) = if extracted.object_is_entity {
        let ot = object_type.unwrap_or(EntityType::Concept);
        let obj = resolve_entity(kg, &extracted.object, ot).await?;
        (Some(obj.id), None)
    } else {
        (None, Some(extracted.object.clone()))
    };

    // 6. Ensure predicate.
    let predicate_id = kg.ensure_predicate(&extracted.predicate).await?;

    // 7. Dedup / corroboration check (stub for #79).
    if let Some(existing_id) = find_existing_fact(
        kg,
        subject.id,
        predicate_id,
        object_id,
        object_literal.as_deref(),
        valid_from,
        valid_until,
    )
    .await?
    {
        return Ok(ProcessResult::Corroborated(existing_id));
    }

    // 8. Map classification to source + confidence.
    let source_type = source_type_for(extracted.classification);
    let confidence = confidence_for(extracted.classification);

    // 9. Handle corrections.
    let mut new_fact = NewFact {
        subject_id: subject.id,
        predicate: extracted.predicate.clone(),
        object_id,
        object_literal,
        valid_from,
        valid_until,
        source_type,
        connector_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: Some(crate::models::source::ExtractionMethod::LlmExtraction),
        inferred: false,
        inference_depth: 0,
        confidence: Some(confidence),
        parent_fact_ids: Vec::new(),
    };

    if extracted.classification == Classification::Correction {
        handle_correction(kg, &extracted, subject.id, predicate_id, &mut new_fact, now).await?;
    }

    // 10. Insert fact.
    let fact = kg.insert_fact(new_fact).await?;

    // 11. If sensitive, mark pending confirmation.
    if extracted.is_sensitive {
        // Override status to Disputed and set pending flag.
        let mut tx = kg.pool().begin().await?;
        sqlx::query(
            "UPDATE facts SET fact_status_id = ?, pending_confirmation = TRUE, updated_at = ? WHERE id = ?",
        )
        .bind(FactStatus::Disputed as i16)
        .bind(now)
        .bind(fact.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        kg.pending_confirmations().write().await.insert(fact.id);

        return Ok(ProcessResult::Pending(PendingFact {
            fact_id: fact.id,
            subject_name: extracted.subject,
            predicate: extracted.predicate,
            object_display: extracted.object,
        }));
    }

    Ok(ProcessResult::Inserted(fact))
}

async fn handle_correction(
    kg: &KnowledgeGraph,
    extracted: &ExtractedFact,
    subject_id: i32,
    predicate_id: i16,
    new_fact: &mut NewFact,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    let scope = extracted.correction_scope.as_deref();

    match scope {
        Some("always") => {
            // Retrospective correction: old fact was never true.
            let overlapping = find_active_overlapping(
                kg,
                subject_id,
                predicate_id,
                new_fact.valid_from,
                new_fact.valid_until,
            )
            .await?;

            let mut tx = kg.pool().begin().await?;
            let mut all_children: Vec<(i32, bool)> = Vec::new();

            for old in overlapping {
                // Mark as Corrected.
                queries::fact::set_status_tx(
                    &mut tx,
                    old.id,
                    FactStatus::Corrected,
                    now,
                    ChangedBy::System,
                )
                .await?;

                // Move to trash (soft-delete with cascade).
                let children =
                    crate::forget::forget_fact_tx(&mut tx, old.id, ChangedBy::System, now).await?;
                all_children.extend(children);
            }

            tx.commit().await?;

            // Deduplicate children so each orphan is evaluated once.
            let mut seen = std::collections::HashSet::new();
            all_children.retain(|(id, _)| seen.insert(*id));

            crate::forget::evaluate_children(kg.pool(), all_children, now).await?;
        }
        Some(datetime_str) => {
            // Temporal correction: parse the datetime and use it as valid_from.
            if let Ok(dt) = DateTime::parse_from_rfc3339(datetime_str) {
                new_fact.valid_from = Some(dt.with_timezone::<Utc>(&Utc));
            }
            // The existing insert_fact temporal-overlap logic will close the
            // sole open-ended predecessor at this datetime automatically.
        }
        None => {
            // No scope provided: default to temporal correction with now().
            new_fact.valid_from = Some(now);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Confirmation helpers
// ---------------------------------------------------------------------------

/// Confirm a pending sensitive fact: flip to Active with confidence 1.0.
pub async fn confirm_fact(kg: &KnowledgeGraph, fact_id: i32) -> Result<Fact, KnowledgeError> {
    let now = kg.now();

    let mut tx = kg.pool().begin().await?;

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
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
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
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
pub async fn reject_fact(kg: &KnowledgeGraph, fact_id: i32) -> Result<(), KnowledgeError> {
    let now = kg.now();

    let mut tx = kg.pool().begin().await?;

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
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
    .bind(Some("User rejected sensitive fact"))
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
