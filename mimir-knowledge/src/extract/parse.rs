//! LLM-output parsing: `RememberOutput` validation, fact conversion,
//! and temporal / recurrence / event / location parsing helpers.

use chrono::{DateTime, Utc};

use mimir_core::llm::types::Message;

use crate::KnowledgeError;
use crate::extract::schema::{Classification, ExtractedFact, ExtractedLocation, RememberOutput};
use crate::models::entity::EntityType;
use crate::models::enums::{EventType, LocationType, RecurrenceType};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::normalize::{NormalizedFact, NormalizedLocation};

// ---------------------------------------------------------------------------

/// Parse the assistant message into a `RememberOutput`.
///
/// Supports both tool-call output and fallback JSON parsing for backends that
/// do not reliably emit structured tool calls.
pub(super) fn parse_remember_output(
    assistant_msg: Message,
) -> Result<RememberOutput, KnowledgeError> {
    if let Some(tool_calls) = assistant_msg.tool_calls {
        let first_call = tool_calls.into_iter().next().ok_or_else(|| {
            KnowledgeError::Validation("LLM tool call list was empty.".to_string())
        })?;

        return serde_json::from_str(&first_call.function.arguments).map_err(|e| {
            KnowledgeError::Validation(format!("Failed to parse tool arguments: {}", e))
        });
    }

    let text = assistant_msg.content.trim();
    if text.is_empty() {
        return Err(KnowledgeError::Validation(
            "LLM did not emit a tool call.".to_string(),
        ));
    }

    let json_text = if text.starts_with("```") {
        text.lines()
            .skip_while(|l| l.starts_with("```"))
            .take_while(|l| !l.starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text.to_string()
    };

    let json_text = json_text.trim();

    if let Ok(wrapper) = serde_json::from_str::<RememberOutput>(json_text) {
        Ok(wrapper)
    } else if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(json_text) {
        Ok(RememberOutput { facts })
    } else {
        Err(KnowledgeError::Validation(format!(
            "LLM did not emit a tool call and response could not be parsed as JSON: {}",
            json_text.chars().take(200).collect::<String>()
        )))
    }
}

// ---------------------------------------------------------------------------
// Classification → SourceType (chat adapter)
// ---------------------------------------------------------------------------

/// Map an LLM [`Classification`] to the [`SourceType`] carried on the
/// [`NormalizedFact`]. Explicit statements and corrections are user edits
/// (confidence 1.0); casual mentions are interactions (confidence 0.30).
pub(super) fn source_type_for(classification: Classification) -> SourceType {
    match classification {
        Classification::Explicit | Classification::Correction => SourceType::UserEdit,
        Classification::Casual => SourceType::Interaction,
    }
}

// ---------------------------------------------------------------------------
// LLM-output parsing helpers (entity types, temporal, recurrence, categories)
// ---------------------------------------------------------------------------

/// Parse an entity type string into the Rust enum.
pub fn parse_entity_type(s: &str) -> Result<EntityType, KnowledgeError> {
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
pub(super) fn split_list_objects(fact: &ExtractedFact) -> Vec<ExtractedFact> {
    let canon = fact.relationship_type.as_str();
    if !LIST_PREDICATES.contains(&canon) {
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
        f.relationship_type = canon.to_string();
        result.push(f);
    }

    if result.is_empty() {
        vec![fact.clone()]
    } else {
        result
    }
}

pub(super) fn parse_extracted_fact(
    extracted: &ExtractedFact,
) -> Result<NormalizedFact, KnowledgeError> {
    let subject_type = parse_entity_type(&extracted.subject_type)?;
    let object_type = extracted
        .object_type
        .as_ref()
        .map(|s| parse_entity_type(s))
        .transpose()?;

    let valid_from = parse_temporal_bound(
        extracted
            .temporal
            .as_ref()
            .and_then(|t| t.valid_from.as_deref()),
    );
    let valid_until = parse_temporal_bound(
        extracted
            .temporal
            .as_ref()
            .and_then(|t| t.valid_until.as_deref()),
    );

    let recurrence = extracted
        .recurrence
        .as_deref()
        .and_then(parse_recurrence)
        .unwrap_or(RecurrenceType::None);
    let requires_user_action = extracted.requires_user_action.unwrap_or(false);

    // Categories arrive as strings from the LLM; parse to IDs, warning on any
    // that are not valid integers (the shared boundary validates them against
    // the DB). This preserves the previous per-fact warning behaviour.
    let mut category_ids = Vec::new();
    for cat_str in &extracted.categories {
        match cat_str.parse::<i32>() {
            Ok(id) => category_ids.push(id),
            Err(_) => tracing::warn!(
                "LLM suggested invalid category '{}' for fact '{} {} {}'; ignoring",
                cat_str,
                extracted.subject,
                extracted.relationship_type,
                extracted.object
            ),
        }
    }

    // Optional structured location overlay (Phase 3 S3 / #193). A malformed
    // overlay is warned and dropped rather than aborting the fact, matching the
    // per-fact tolerance used for categories.
    let location = extracted
        .location
        .as_ref()
        .and_then(|loc| match parse_location(loc) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                tracing::warn!(
                    "invalid location overlay for fact '{} {} {}'; ignoring: {error}",
                    extracted.subject,
                    extracted.relationship_type,
                    extracted.object
                );
                None
            }
        });

    Ok(NormalizedFact {
        source_type: source_type_for(extracted.classification),
        subject: extracted.subject.clone(),
        subject_type,
        relationship_type: extracted.relationship_type.clone(),
        object: extracted.object.clone(),
        object_is_entity: extracted.object_is_entity,
        object_type,
        valid_from,
        valid_until,
        is_sensitive: extracted.is_sensitive,
        is_correction: extracted.classification == Classification::Correction,
        correction_scope: extracted.correction_scope.clone(),
        category_ids,
        recurrence,
        requires_user_action,
        // Conversational facts have no native source item id.
        raw_reference: None,
        // Conversational facts are LLM-extracted; carry that on the fact so
        // the value survives even if a future caller mixes methods in a batch.
        extraction_method: Some(ExtractionMethod::LlmExtraction),
        // Chat never hints the event kind; the overlay derives it.
        event_type: None,
        location,
    })
}

/// Parse an RFC-3339 temporal bound, warning and dropping it on failure so a
/// malformed bound never aborts the whole fact (matches the legacy behaviour).
pub fn parse_temporal_bound(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?;
    match DateTime::parse_from_rfc3339(s) {
        Ok(dt) => Some(dt.with_timezone::<Utc>(&Utc)),
        Err(e) => {
            tracing::warn!(
                "Failed to parse temporal bound '{}': {}. Temporal constraint ignored.",
                s,
                e
            );
            None
        }
    }
}

/// Map an LLM-emitted recurrence string to a `RecurrenceType`.
pub fn parse_recurrence(value: &str) -> Option<RecurrenceType> {
    match value.to_ascii_lowercase().as_str() {
        "none" => Some(RecurrenceType::None),
        "daily" => Some(RecurrenceType::Daily),
        "weekly" => Some(RecurrenceType::Weekly),
        "monthly" => Some(RecurrenceType::Monthly),
        "yearly" => Some(RecurrenceType::Yearly),
        _ => None,
    }
}

/// Map an LLM-emitted event-type string to an [`EventType`].
///
/// Connector LLM extraction (Email C7 / #201) includes an optional event
/// hint in its tool output; Rust validates it against the enum rather
/// than trusting the raw string, returning `None` for an unrecognised
/// value so the events-subsystem overlay falls back to derivation.
pub fn parse_event_type(value: &str) -> Option<EventType> {
    match value.trim() {
        "Birthday" => Some(EventType::Birthday),
        "Appointment" => Some(EventType::Appointment),
        "Deadline" => Some(EventType::Deadline),
        "Task" => Some(EventType::Task),
        "Reminder" => Some(EventType::Reminder),
        "Custom" => Some(EventType::Custom),
        _ => None,
    }
}

/// Map an LLM-emitted location-type string to a [`LocationType`].
pub fn parse_location_type(value: &str) -> Result<LocationType, KnowledgeError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "home" => Ok(LocationType::Home),
        "work" => Ok(LocationType::Work),
        "visited" => Ok(LocationType::Visited),
        "origin" => Ok(LocationType::Origin),
        "current" => Ok(LocationType::Current),
        other => Err(KnowledgeError::Validation(format!(
            "unknown location_type {other:?}"
        ))),
    }
}

/// Parse an [`ExtractedLocation`] overlay into a [`NormalizedLocation`].
///
/// `location_type` is required (it classifies the row); the geo half
/// (address / coords) is optional and filled by the geocoder later when only
/// one side is known.
pub fn parse_location(loc: &ExtractedLocation) -> Result<NormalizedLocation, KnowledgeError> {
    let location_type = match loc.location_type.as_deref() {
        Some(s) => parse_location_type(s)?,
        None => {
            return Err(KnowledgeError::Validation(
                "location overlay missing location_type".to_string(),
            ));
        }
    };
    Ok(NormalizedLocation {
        location_type,
        address: loc.address.clone(),
        latitude: loc.latitude,
        longitude: loc.longitude,
        timezone: loc.timezone.clone(),
    })
}
