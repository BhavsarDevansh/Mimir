//! LLM tool schema and system prompt for prose fact extraction.

use std::sync::LazyLock;

use mimir_knowledge::extract::{ExtractedLocation, Temporal};
/// Name of the LLM tool the extractor must call. Kept as a single constant so
/// the schema and the response-validation step agree on the expected name; a
/// tool call whose `function.name` differs is rejected (see [`parse_output`]).
pub(super) const EMAIL_EXTRACTION_TOOL_NAME: &str = "extract_email_facts";

// ---------------------------------------------------------------------------
// Wire types (LLM tool output)
// ---------------------------------------------------------------------------

/// One fact emitted by the LLM for a single email. Mirrors the conversational
/// [`mimir_knowledge::extract::ExtractedFact`] minus the conversational-only
/// fields (`classification`, `correction_scope`), plus an optional
/// `event_type` hint that Rust maps onto [`NormalizedFact::event_type`].
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(super) struct EmailFact {
    pub(super) subject: String,
    pub(super) subject_type: String,
    pub(super) relationship_type: String,
    pub(super) object: String,
    pub(super) object_is_entity: bool,
    #[serde(default)]
    pub(super) object_type: Option<String>,
    #[serde(default)]
    pub(super) temporal: Option<Temporal>,
    /// Event kind hint (Birthday / Appointment / Deadline / Task / Reminder /
    /// Custom). Validated against the [`EventType`] enum in Rust; an
    /// unrecognised value is dropped (the overlay derives the type).
    #[serde(default)]
    pub(super) event_type: Option<String>,
    #[serde(default)]
    pub(super) recurrence: Option<String>,
    #[serde(default)]
    pub(super) requires_user_action: Option<bool>,
    #[serde(default)]
    pub(super) is_sensitive: bool,
    #[serde(default)]
    pub(super) category_ids: Vec<i32>,
    #[serde(default)]
    pub(super) location: Option<ExtractedLocation>,
}

/// Wrapper the tool returns: an empty `facts` array means the email carried no
/// real-world facts (marketing, newsletter, or nothing actionable).
#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct EmailFactOutput {
    pub(super) facts: Vec<EmailFact>,
}

/// The `extract_email_facts` tool JSON Schema, built once and shared by every
/// email extraction call (issue #259). The schema is static — there is no
/// per-call input — so rebuilding it per email was a steady stream of
/// identical allocations during a long sync.
static EMAIL_EXTRACTION_TOOL_TEMPLATE: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": EMAIL_EXTRACTION_TOOL_NAME,
            "description": "Extract real-world facts about the user that the email's prose conveys. Do NOT model the email itself as a fact; extract the underlying event, booking, date, address, transaction, or commitment. Return an empty facts array for marketing, newsletters, or emails with no usable facts.",
            "parameters": {
                "type": "object",
                "properties": {
                    "facts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "subject": {
                                    "type": "string",
                                    "description": "The entity the fact is about. For facts about the mailbox owner, use their name exactly as given in the task."
                                },
                                "subject_type": {
                                    "type": "string",
                                    "enum": ["Person","Place","Event","Object","Concept","Organization","Activity","DateTime"],
                                    "description": "Entity type of the subject."
                                },
                                "relationship_type": {
                                    "type": "string",
                                    "description": "The relationship or property being asserted. Must be one of the canonical predicates listed in the system prompt (the full vocabulary is listed there); a fact with any other predicate is staged for review."
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
                                    "enum": ["Person","Place","Event","Object","Concept","Organization","Activity","DateTime"],
                                    "description": "Entity type of the object, if object_is_entity is true."
                                },
                                "temporal": {
                                    "type": "object",
                                    "properties": {
                                        "valid_from": {"type": "string", "description": "ISO-8601 datetime when this fact becomes true."},
                                        "valid_until": {"type": "string", "description": "ISO-8601 datetime when this fact ceases to be true."}
                                    }
                                },
                                "event_type": {
                                    "type": "string",
                                    "enum": ["Birthday","Appointment","Deadline","Task","Reminder","Custom"],
                                    "description": "Optional event kind hint for timed/recurring/action items. Omit for non-event facts."
                                },
                                "recurrence": {
                                    "type": "string",
                                    "enum": ["none","daily","weekly","monthly","yearly"],
                                    "description": "How the date recurs, for recurring facts (birthdays, anniversaries). Omit or 'none' for one-time facts."
                                },
                                "requires_user_action": {
                                    "type": "boolean",
                                    "description": "True for tasks/deadlines the user must complete. False or omit for reminders that auto-complete."
                                },
                                "is_sensitive": {
                                    "type": "boolean",
                                    "description": "Whether this fact involves health, financial, relationship, or other sensitive topics."
                                },
                                "category_ids": {
                                    "type": "array",
                                    "items": { "type": "integer" },
                                    "description": "Dewey Decimal category IDs from the Categorisation Guide that best describe this fact. Rust validates IDs and supplies a taxonomy fallback when none are valid."
                                },
                                "location": {
                                    "type": "object",
                                    "description": "Optional. Present only for 'where' facts.",
                                    "properties": {
                                        "location_type": {"type": "string", "enum": ["Home","Work","Visited","Origin","Current"]},
                                        "address": {"type": "string"},
                                        "latitude": {"type": "number"},
                                        "longitude": {"type": "number"},
                                        "timezone": {"type": "string"}
                                    },
                                    "required": ["location_type"]
                                }
                            },
                            "required": ["subject","subject_type","relationship_type","object","object_is_entity"]
                        }
                    }
                },
                "required": ["facts"]
            }
        }
    })
});

pub(super) fn email_extraction_tool_schema(predicate_names: &[String]) -> serde_json::Value {
    let mut schema = EMAIL_EXTRACTION_TOOL_TEMPLATE.clone();
    let relationship_type = &mut schema["function"]["parameters"]["properties"]["facts"]["items"]["properties"]
        ["relationship_type"];
    relationship_type["enum"] = serde_json::Value::Array(
        predicate_names
            .iter()
            .map(|name| serde_json::Value::String(name.clone()))
            .collect(),
    );
    schema
}

/// The controlled relationship-type vocabulary the model must stay within,
/// rendered from the DB-derived emit-eligible leaf list also used by the Rust
/// validator, so the prompt and the validator cannot drift apart.
pub(super) fn build_system_prompt(
    user_identity: Option<&str>,
    predicate_names: &[String],
) -> String {
    let owner = user_identity.unwrap_or("the mailbox owner");
    let vocabulary = predicate_names.join(", ");
    format!(
        "You are Mimir's email fact extractor. Read the provided email and \
extract the real-world facts it conveys about {owner} — appointments, flights, \
bookings, deadlines, tasks, addresses, dates, financial transactions, job \
offers, and other concrete facts about {owner}. Extract the underlying \
event or thing, not the email itself (do not emit 'received email from' \
facts). If the email is marketing, a newsletter, or carries no real-world \
facts about {owner}, return an empty facts array. For facts about {owner}, \
use the exact name '{owner}' as the subject. Emit the facts via the \
extract_email_facts tool.\n\n\
Predicate vocabulary: the relationship_type enum in the extraction tool is \
closed. A fact whose predicate is not in this vocabulary is staged for review \
and never reaches the knowledge graph:\n{vocabulary}"
    )
}
