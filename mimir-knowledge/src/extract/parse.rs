//! LLM-output parsing: `RememberOutput` validation, fact conversion,
//! and temporal / recurrence / event / location parsing helpers.

use chrono::{DateTime, Utc};

use mimir_core::llm::types::Message;
use mimir_core::llm::{ToolOutputParseError, parse_tool_output};

use crate::KnowledgeError;
use crate::extract::schema::{Classification, ExtractedFact, ExtractedLocation, RememberOutput};
use crate::models::entity::EntityType;
use crate::models::enums::{EventType, LocationType, RecurrenceType};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::normalize::{NormalizedFact, NormalizedLocation};
use crate::{MULTI_VALUED_PREDICATES, is_favourite_family_predicate};

// ---------------------------------------------------------------------------

/// Parse the assistant message into a `RememberOutput`.
///
/// Supports both tool-call output and fallback JSON parsing for backends that
/// do not reliably emit structured tool calls. The tool-call + fence-fallback
/// dance is shared with the connector extraction path via
/// [`parse_tool_output`]; the conversational-only bare-`Vec<ExtractedFact>`
/// fallback is applied here on top.
pub(super) fn parse_remember_output(
    assistant_msg: Message,
) -> Result<RememberOutput, KnowledgeError> {
    match parse_tool_output::<RememberOutput>(assistant_msg, None) {
        Ok(wrapper) => Ok(wrapper),
        // The conversational path also accepts a bare `Vec<ExtractedFact>`
        // (no `{"facts": [...]}` wrapper) from backends that skip the wrapper.
        Err(ToolOutputParseError::InvalidJson { head, text }) => {
            if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(&text) {
                return Ok(RememberOutput { facts });
            }
            Err(KnowledgeError::Validation(format!(
                "LLM did not emit a tool call and response could not be parsed as JSON: {}",
                head
            )))
        }
        Err(error) => Err(KnowledgeError::Validation(error.to_string())),
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
///
/// Delegates to [`EntityType`]'s `FromStr` so the LLM extraction validation
/// shares the enum's single wire-string table (issue #358).
pub fn parse_entity_type(s: &str) -> Result<EntityType, KnowledgeError> {
    s.parse()
        .map_err(|_| KnowledgeError::Validation(format!("Invalid entity_type: {}", s)))
}

// ---------------------------------------------------------------------------
// List splitting
// ---------------------------------------------------------------------------

/// If a fact has a comma-separated object literal and its predicate is
/// multi-valued — in the shared [`MULTI_VALUED_PREDICATES`] allow-list, or the
/// open `favourite_<thing>` family — expand it into multiple
/// `ExtractedFact`s.
///
/// Splitting is a best-effort pass on simple commas: the prompt already
/// instructs the model to emit one fact per list item, and the splitter is the
/// deterministic safety net for crammed lists. Predicates outside the
/// multi-valued set and the open favourite family pass through untouched.
pub(super) fn split_list_objects(fact: &ExtractedFact) -> Vec<ExtractedFact> {
    let canon = fact.relationship_type.as_str();
    if !MULTI_VALUED_PREDICATES.contains(&canon) && !is_favourite_family_predicate(canon) {
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
        confidence: None,
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
        // LLM extraction emits the recurrence kind only — no RRULE, interval,
        // or series bounds (the connector extractors supply those).
        recurrence_rule: None,
        recurrence_interval: 1,
        recurrence_until: None,
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
    value.parse().ok()
}

/// Map an LLM-emitted event-type string to an [`EventType`].
///
/// Connector LLM extraction (Email C7 / #201) includes an optional event
/// hint in its tool output; Rust validates it against the enum rather
/// than trusting the raw string, returning `None` for an unrecognised
/// value so the events-subsystem overlay falls back to derivation.
pub fn parse_event_type(value: &str) -> Option<EventType> {
    value.parse().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use mimir_core::llm::types::{FunctionCall, ToolCall};

    fn tool_call_message(arguments: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![ToolCall {
                index: 0,
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "remember".to_string(),
                    arguments: arguments.to_string(),
                },
            }]),
            tool_call_id: None,
        }
    }

    fn content_message(content: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn fact_json() -> serde_json::Value {
        serde_json::json!({
            "classification": "Explicit",
            "subject": "devansh",
            "subject_type": "Person",
            "relationship_type": "favourite_colour",
            "object": "green",
            "object_is_entity": false,
            "is_sensitive": false
        })
    }

    #[test]
    fn tool_call_arguments_parse_into_remember_output() {
        let msg = tool_call_message(&serde_json::json!({ "facts": [fact_json()] }).to_string());
        let out = parse_remember_output(msg).expect("tool call parses");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].object, "green");
    }

    #[test]
    fn fenced_content_fallback_parses_wrapper() {
        let text = format!(
            "```json\n{}\n```",
            serde_json::json!({ "facts": [fact_json()] })
        );
        let out = parse_remember_output(content_message(&text)).expect("fenced wrapper parses");
        assert_eq!(out.facts.len(), 1);
    }

    #[test]
    fn bare_fact_array_fallback_is_wrapped() {
        let text = serde_json::json!([fact_json()]).to_string();
        let out = parse_remember_output(content_message(&text)).expect("bare array parses");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].object, "green");
    }

    #[test]
    fn invalid_content_is_a_validation_error() {
        let err = parse_remember_output(content_message("not json at all"))
            .expect_err("invalid content rejected");
        assert!(matches!(err, KnowledgeError::Validation(_)));
    }

    #[test]
    fn split_list_objects_splits_favourite_family() {
        let fact = ExtractedFact {
            classification: Classification::Explicit,
            subject: "devansh".to_string(),
            subject_type: "Person".to_string(),
            relationship_type: "favourite_movie".to_string(),
            object: "Inception, Interstellar".to_string(),
            object_is_entity: false,
            object_type: None,
            temporal: None,
            is_sensitive: false,
            correction_scope: None,
            categories: vec![],
            recurrence: None,
            requires_user_action: None,
            location: None,
        };

        let split = split_list_objects(&fact);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].object, "Inception");
        assert_eq!(split[1].object, "Interstellar");
        assert_eq!(split[0].relationship_type, "favourite_movie");
    }
}
