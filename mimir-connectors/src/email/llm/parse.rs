//! LLM-output parsing and typed-fact construction with Rust-side validation.
use mimir_core::llm::Message as LlmMessage;
use mimir_core::llm::parse_tool_output;
use mimir_knowledge::extract::{
    parse_entity_type, parse_event_type, parse_location, parse_recurrence, parse_temporal_bound,
};
use mimir_knowledge::models::enums::RecurrenceType;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::NormalizedFact;
use tracing::warn;

use crate::connector::ConnectorError;
use crate::email::llm::message::canonicalise_subject;
use crate::email::llm::schema::{EMAIL_EXTRACTION_TOOL_NAME, EmailFact, EmailFactOutput};

pub(super) fn parse_output(message: LlmMessage) -> Result<EmailFactOutput, ConnectorError> {
    // The tool-call + fence-fallback parsing is shared with the
    // conversational `remember` path (issue #259). The expected-tool-name
    // guard keeps the email-specific validation: a single email needs exactly
    // one `extract_email_facts` call, and arguments from a different function
    // must never become email facts.
    parse_tool_output::<EmailFactOutput>(message, Some(EMAIL_EXTRACTION_TOOL_NAME))
        .map_err(|e| ConnectorError::Parse(e.to_string()))
}

/// Build a single [`NormalizedFact`] from one LLM-emitted [`EmailFact`].
/// Per-field validation matches the conversational path: an invalid entity
/// type / event type / location is warned and dropped (or the whole fact
pub(super) fn build_fact(
    fact: EmailFact,
    user_identity: Option<&str>,
    raw_ref: &str,
) -> Result<NormalizedFact, ConnectorError> {
    let subject_type = parse_entity_type(&fact.subject_type).map_err(|e| {
        ConnectorError::Parse(format!("invalid subject_type {:?}: {e}", fact.subject_type))
    })?;
    let object_type = fact
        .object_type
        .as_deref()
        .map(parse_entity_type)
        .transpose()
        .map_err(|e| ConnectorError::Parse(format!("invalid object_type: {e}")))?;

    let valid_from =
        parse_temporal_bound(fact.temporal.as_ref().and_then(|t| t.valid_from.as_deref()));
    let valid_until = parse_temporal_bound(
        fact.temporal
            .as_ref()
            .and_then(|t| t.valid_until.as_deref()),
    );
    let recurrence = fact
        .recurrence
        .as_deref()
        .and_then(parse_recurrence)
        .unwrap_or(RecurrenceType::None);
    let requires_user_action = fact.requires_user_action.unwrap_or(false);

    // Event-type hint validated against the enum; unrecognised → None (the
    // overlay derives the type). Never trusted raw.
    let event_type = fact.event_type.as_deref().and_then(parse_event_type);
    if fact.event_type.is_some() && event_type.is_none() {
        warn!(
            raw_ref,
            "LLM emitted unrecognised event_type {:?}; dropping hint", fact.event_type
        );
    }

    let location = fact
        .location
        .as_ref()
        .and_then(|loc| match parse_location(loc) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                warn!(
                    raw_ref,
                    "invalid location overlay for email fact; ignoring: {error}"
                );
                None
            }
        });

    Ok(NormalizedFact {
        source_type: SourceType::Connector,
        subject: canonicalise_subject(&fact.subject, user_identity),
        subject_type,
        relationship_type: fact.relationship_type,
        object: fact.object,
        object_is_entity: fact.object_is_entity,
        object_type,
        valid_from,
        valid_until,
        is_sensitive: fact.is_sensitive,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence,
        requires_user_action,
        raw_reference: Some(raw_ref.to_string()),
        extraction_method: Some(ExtractionMethod::LlmExtraction),
        event_type,
        location,
    })
}
