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
    pub relationship_type: String,
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
    #[serde(default)]
    pub categories: Vec<String>,
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
    pub relationship_type: String,
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
/// Build the JSON Schema for the `remember` tool.
pub fn remember_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "remember",
            "description": "Extract structured facts from user messages. Each fact is a subject-relationship_type-object triple with classification, temporal bounds, and sensitivity flags.",
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
                                "relationship_type": {
                                    "type": "string",
                                    "description": "The relationship or property being asserted."
                                },
                                "object": {
                                    "type": "string",
                                    "description": "The value or target of the relationship_type."
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
                                },
                                "categories": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Dewey Decimal category IDs (e.g., ['200', '210']) that best describe the topic of this fact. Use the Categorisation Guide in the system prompt."
                                }
                            },
                            "required": ["classification", "subject", "subject_type", "relationship_type", "object", "object_is_entity"]
                        }
                    }
                },
                "required": ["facts"]
            }
        }
    })
}

/// Return just the inner `parameters` schema for use with the `Tool` trait.
pub fn remember_tool_params_schema() -> serde_json::Value {
    remember_tool_schema()["function"]["parameters"].clone()
}

/// Build the system prompt for fact extraction, including the category taxonomy.
async fn build_extraction_prompt(kg: &KnowledgeGraph) -> Result<String, KnowledgeError> {
    let roots = kg.list_categories(None).await?;
    let mut guide = String::from("Categorisation Guide:\n");
    for root in roots {
        guide.push_str(&format!("{} {}\n", root.id, root.name));
        let children = kg.list_categories(Some(root.id)).await?;
        for child in children {
            guide.push_str(&format!("  {} {}\n", child.id, child.name));
        }
    }

    Ok(format!(
        "You are a fact extractor. Read the user message and emit structured facts via the 'remember' tool.\n\n### Rules\n- Classify each fact as Explicit, Casual, or Correction.\n- For Corrections, set correction_scope to 'always' or an ISO-8601 datetime.\n- Mark health, financial, relationship, religious, political, or legal facts as is_sensitive=true.\n- Subject and object types must be one of: Person, Place, Event, Object, Concept, Organization, Activity, DateTime.\n- Assign 1-3 category IDs from the guide below to each fact. Use the MOST specific sub-category available.\n{}\n### Predicate standards (critical)\nUse the EXACT predicate name below for the matching scenario. Do NOT invent synonyms.\n- Education\n  * Where someone studied   → studied_at (NOT 'attended')\n  * What someone studied    → studied\n  * Degree completed        → completed_degree\n  * Degree status           → educational_status\n- Employment\n  * Employer                → works_at\n  * Job title               → job_title\n  * Profession              → works_as\n- Residence\n  * Current city/country    → based_in\n  * Previous city           → lived_in\n- Personal\n  * Hobby (one per fact)    → hobby (NOT 'hobbies')\n  * Favourite thing         → favourite_{{thing}}\n  * Name                    → has_name\n  * Preferred name          → preferred_name\n  * Pet ownership           → has_pets\n- Family\n  * Sibling                 → has_sibling\n  * Partner                 → has_partner\n  * Parent                  → has_parent\n  * Child                   → has_child\n### Splitting lists\nWhen a user lists multiple items for the same predicate, emit ONE fact PER item.\nBAD (one fact):  hobby → 'Geopolitics, Software Development, Tech'\nGOOD (three facts):\n  hobby → 'Geopolitics'\n  hobby → 'Software Development'\n  hobby → 'Tech'\n### Deduplication\nBefore emitting a fact, ask yourself: 'Have I already emitted a fact with the same subject and the same meaning?' If yes, do not emit the duplicate — instead strengthen the confidence by marking it Explicit.\nExample: If you already emitted studied_at='University of Auckland', do NOT also emit attended='University of Auckland'.\n### Output\nEmit ONLY via the 'remember' tool. Do not output free text.",
        guide
    ))
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
// Predicate normalisation
// ---------------------------------------------------------------------------

/// Map common LLM predicate synonyms to canonical names.
fn normalize_predicate(pred: &str) -> String {
    let trimmed = pred.trim();
    let lowered = trimmed.to_lowercase().replace(' ', "_");
    match lowered.as_str() {
        "attended" | "went_to" | "graduated_from" | "alumni_of" => "studied_at".to_string(),
        "hobbies" | "interests" => "hobby".to_string(),
        "likes" => "likes".to_string(),
        "dislikes" => "dislikes".to_string(),
        "works_for" | "employer" => "works_at".to_string(),
        "profession" | "occupation" => "works_as".to_string(),
        "resides_in" | "current_city" => "based_in".to_string(),
        "previously_lived_in" | "former_city" => "lived_in".to_string(),
        "pet" | "pets" | "owns_pet" => "has_pets".to_string(),
        "brother" | "sister" | "siblings" => "has_sibling".to_string(),
        "spouse" | "boyfriend" | "girlfriend" | "partner" | "wife" | "husband" => {
            "has_partner".to_string()
        }
        "father" | "mother" | "parents" => "has_parent".to_string(),
        "son" | "daughter" | "children" => "has_child".to_string(),
        "name" => "has_name".to_string(),
        "nickname" | "nick_name" | "called" | "goes_by" => "preferred_name".to_string(),
        "favorite_food" | "fav_food" | "favourite_food" => "favourite_food".to_string(),
        "favorite_colour" | "favorite_color" | "fav_color" | "fav_colour" | "color" | "colour" => {
            "favourite_colour".to_string()
        }
        "food_allergy" | "medical_condition" | "condition" => "health_condition".to_string(),
        _ => lowered,
    }
}

// ---------------------------------------------------------------------------
// List splitting
// ---------------------------------------------------------------------------

/// Predicates that typically represent a collection of independent values.
const LIST_PREDICATES: [&str; 11] = [
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

/// If a fact has a comma-separated object literal and its predicate is in the
/// `LIST_PREDICATES` allow-list, expand it into multiple `ExtractedFact`s.
///
/// We only split on simple commas to avoid breaking phrases like
/// "Manchester, UK" — that predicate won't be in the allow-list anyway.
fn split_list_objects(fact: &ExtractedFact) -> Vec<ExtractedFact> {
    let canon = normalize_predicate(&fact.relationship_type);
    if !LIST_PREDICATES.contains(&canon.as_str()) {
        return vec![fact.clone()];
    }
    let parts: Vec<&str> = fact.object.split(',').collect();
    if parts.len() <= 1 {
        return vec![fact.clone()];
    }

    let mut result = Vec::with_capacity(parts.len());
    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut f = fact.clone();
        f.object = trimmed.to_string();
        f.relationship_type = canon.clone();
        result.push(f);
    }

    if result.is_empty() {
        vec![fact.clone()]
    } else {
        result
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
    relationship_type_id: i16,
    object_id: Option<i32>,
    object_literal: Option<&str>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> Result<Option<i32>, KnowledgeError> {
    let rows: Vec<(i32,)> = sqlx::query_as(
        "SELECT id FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ? \
         AND object_id IS ? AND object_literal IS ? \
         AND (fact_status_id = ? OR pending_confirmation = TRUE) \
         AND ((valid_from IS ?) OR (valid_from = ?)) \
         AND ((valid_until IS ?) OR (valid_until = ?))",
    )
    .bind(subject_id)
    .bind(relationship_type_id)
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

/// Find active facts with the same subject + relationship_type that overlap the new fact.
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
    let prompt = build_extraction_prompt(kg).await?;
    let messages = vec![Message::system(prompt), Message::user(user_message)];
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

    let mut all_facts: Vec<ExtractedFact> = Vec::new();
    for fact in extracted.facts {
        let expanded = split_list_objects(&fact);
        all_facts.extend(expanded);
    }

    let mut outcome = ExtractionOutcome::default();

    for fact in all_facts {
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

pub(crate) enum ProcessResult {
    Inserted(Fact),
    Pending(PendingFact),
    Corroborated(i32),
}

/// Insert a sensitive fact atomically with Disputed status and pending_confirmation=TRUE.
async fn insert_sensitive_fact(
    kg: &KnowledgeGraph,
    new_fact: NewFact,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let relationship_type_id = kg
        .ensure_relationship_type(&new_fact.relationship_type)
        .await?;

    // Calculate confidence
    let confidence = new_fact.confidence.unwrap_or_else(|| {
        let source_type = source_type_for(Classification::Explicit);
        crate::confidence::initial(source_type, None)
    });

    let mut tx = kg.pool().begin().await?;

    // Insert with Disputed status and pending_confirmation=TRUE in a single atomic operation.
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
    .bind(true) // pending_confirmation
    .bind(memory_priority_id)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    let fact_id = fact_id as i32;

    // Insert source
    let extraction_method_id = new_fact.extraction_method.map(|e| e as i16);
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

    // Audit log
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

    tx.commit().await?;

    // Fetch the created fact
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

/// Process a `RememberOutput` by applying the same validation, dedup,
/// confidence assignment, and insertion logic used by the full extraction pipeline.
///
/// This is the entrypoint for the `remember` tool: the LLM has already structured
/// the facts, so we only need to validate and persist them.
pub async fn process_remember_output(
    kg: &KnowledgeGraph,
    output: RememberOutput,
) -> Result<ExtractionOutcome, KnowledgeError> {
    let now = kg.now();
    let mut all_facts: Vec<ExtractedFact> = Vec::new();
    for fact in output.facts {
        let expanded = split_list_objects(&fact);
        all_facts.extend(expanded);
    }

    let mut outcome = ExtractionOutcome::default();

    for fact in all_facts {
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

pub(crate) async fn process_extracted_fact(
    kg: &KnowledgeGraph,
    mut extracted: ExtractedFact,
    now: DateTime<Utc>,
) -> Result<ProcessResult, KnowledgeError> {
    // 3. Normalise predicate before any downstream logic.
    extracted.relationship_type = normalize_predicate(&extracted.relationship_type);

    // Validate entity types.
    let subject_type = parse_entity_type(&extracted.subject_type)?;
    let object_type = extracted
        .object_type
        .as_ref()
        .map(|s| parse_entity_type(s))
        .transpose()?;

    // Parse temporal.
    let valid_from = if let Some(temporal) = &extracted.temporal {
        if let Some(s) = &temporal.valid_from {
            match DateTime::parse_from_rfc3339(s) {
                Ok(dt) => Some(dt.with_timezone::<Utc>(&Utc)),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse valid_from temporal bound '{}': {}. Temporal constraint ignored.",
                        s,
                        e
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let valid_until = if let Some(temporal) = &extracted.temporal {
        if let Some(s) = &temporal.valid_until {
            match DateTime::parse_from_rfc3339(s) {
                Ok(dt) => Some(dt.with_timezone::<Utc>(&Utc)),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse valid_until temporal bound '{}': {}. Temporal constraint ignored.",
                        s,
                        e
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Resolve entities.
    let subject = resolve_entity(kg, &extracted.subject, subject_type).await?;
    let (object_id, object_literal) = if extracted.object_is_entity {
        let ot = object_type.unwrap_or(EntityType::Concept);
        let obj = resolve_entity(kg, &extracted.object, ot).await?;
        (Some(obj.id), None)
    } else {
        (None, Some(extracted.object.clone()))
    };

    // If this fact establishes a preferred name, register the object as an alias
    // so future lookups by that short name resolve to the canonical entity.
    if extracted.relationship_type == "preferred_name" {
        let alias = &extracted.object;
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
                    let (fact_count,): (i64,) = sqlx::query_as(
                        "SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?",
                    )
                    .bind(cand.entity.id)
                    .bind(cand.entity.id)
                    .fetch_one(kg.pool())
                    .await
                    .unwrap_or((0,));
                    if fact_count <= 2 {
                        if let Err(e) =
                            queries::entity::auto_merge_pair(kg.pool(), subject.id, cand.entity.id)
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
                    break;
                }
            }
        }
    }

    // Ensure relationship_type.
    let relationship_type_id = kg
        .ensure_relationship_type(&extracted.relationship_type)
        .await?;

    // Dedup / corroboration check (stub for #79).
    if let Some(existing_id) = find_existing_fact(
        kg,
        subject.id,
        relationship_type_id,
        object_id,
        object_literal.as_deref(),
        valid_from,
        valid_until,
    )
    .await?
    {
        return Ok(ProcessResult::Corroborated(existing_id));
    }

    // Map classification to source + confidence.
    let source_type = source_type_for(extracted.classification);
    let confidence = confidence_for(extracted.classification);

    // Validate and collect category IDs.
    let mut category_ids = Vec::new();
    for cat_str in &extracted.categories {
        if let Ok(id) = cat_str.parse::<i32>() {
            match kg.get_category(id).await? {
                Some(_) => category_ids.push(id),
                None => {
                    tracing::warn!(
                        "LLM suggested unknown category {} for fact '{} {} {}'; ignoring",
                        id,
                        extracted.subject,
                        extracted.relationship_type,
                        extracted.object
                    );
                }
            }
        } else {
            tracing::warn!(
                "LLM suggested invalid category '{}' for fact '{} {} {}'; ignoring",
                cat_str,
                extracted.subject,
                extracted.relationship_type,
                extracted.object
            );
        }
    }

    // Handle corrections.
    let mut new_fact = NewFact {
        subject_id: subject.id,
        relationship_type: extracted.relationship_type.clone(),
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
        category_ids,
    };

    if extracted.classification == Classification::Correction {
        handle_correction(
            kg,
            &extracted,
            subject.id,
            relationship_type_id,
            &mut new_fact,
            now,
        )
        .await?;
    }

    // Insert fact, handling sensitive facts atomically.
    if extracted.is_sensitive {
        // For sensitive facts, insert directly with Disputed status and pending_confirmation in one transaction.
        let fact = insert_sensitive_fact(kg, new_fact, now).await?;

        // Only add to in-memory cache after successful commit.
        kg.pending_confirmations().write().await.insert(fact.id);

        return Ok(ProcessResult::Pending(PendingFact {
            fact_id: fact.id,
            subject_name: extracted.subject,
            relationship_type: extracted.relationship_type,
            object_display: extracted.object,
        }));
    }

    // Non-sensitive facts go through the normal path.
    let fact = kg.insert_fact(new_fact).await?;
    Ok(ProcessResult::Inserted(fact))
}

async fn handle_correction(
    kg: &KnowledgeGraph,
    extracted: &ExtractedFact,
    subject_id: i32,
    relationship_type_id: i16,
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
                relationship_type_id,
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
            match DateTime::parse_from_rfc3339(datetime_str) {
                Ok(dt) => {
                    new_fact.valid_from = Some(dt.with_timezone::<Utc>(&Utc));
                    // The existing insert_fact temporal-overlap logic will close the
                    // sole open-ended predecessor at this datetime automatically.
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse correction_scope datetime '{}': {}. valid_from will not be set.",
                        datetime_str,
                        e
                    );
                }
            }
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
