//! The `remember` tool JSON Schema exposed to the LLM.

use std::sync::LazyLock;

// ---------------------------------------------------------------------------

/// The `remember` tool JSON Schema, built once and shared by every extraction
/// call (issue #259). The schema is static — there is no per-call input — so
/// rebuilding it per extraction was a steady stream of identical allocations.
static REMEMBER_TOOL_SCHEMA_TEMPLATE: LazyLock<serde_json::Value> = LazyLock::new(|| {
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
                                    "description": "The controlled relationship leaf being asserted. Unknown predicates are staged for governance review; they are never inserted."
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
                                },
                                "recurrence": {
                                    "type": "string",
                                    "enum": ["none", "daily", "weekly", "monthly", "yearly"],
                                    "description": "For recurring facts (birthdays, anniversaries, routines): how the date recurs. Omit or set 'none' for one-time facts."
                                },
                                "requires_user_action": {
                                    "type": "boolean",
                                    "description": "True for tasks/deadlines the user must complete (the event stays Active past its trigger date). False or omit for reminders that auto-complete when the date passes."
                                },
                                "location": {
                                    "type": "object",
                                    "description": "Optional. Present only for 'where' facts (where the subject lives/works/is located). Carries the structured geo data that becomes an entity location; the temporal bounds on the fact model moves (e.g. home 2020-2023, home 2023-present).",
                                    "properties": {
                                        "location_type": {
                                            "type": "string",
                                            "enum": ["Home", "Work", "Visited", "Origin", "Current"],
                                            "description": "Classification of the location."
                                        },
                                        "address": {
                                            "type": "string",
                                            "description": "Free-text address or place name. Omit when only coordinates are known (Mimir reverse-geocodes them)."
                                        },
                                        "latitude": {
                                            "type": "number",
                                            "description": "WGS-84 latitude in decimal degrees. Omit when only an address is known (Mimir forward-geocodes it)."
                                        },
                                        "longitude": {
                                            "type": "number",
                                            "description": "WGS-84 longitude in decimal degrees."
                                        },
                                        "timezone": {
                                            "type": "string",
                                            "description": "IANA timezone name (e.g. Europe/London), when known."
                                        }
                                    },
                                    "required": ["location_type"]
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
});

/// Build the `remember` tool JSON Schema with the DB taxonomy's emit-eligible
/// leaves as a closed enum.
pub fn remember_tool_schema(predicate_names: &[String]) -> serde_json::Value {
    let mut schema = REMEMBER_TOOL_SCHEMA_TEMPLATE.clone();
    schema["function"]["parameters"]["properties"]["facts"]["items"]["properties"]["relationship_type"]
        ["enum"] = serde_json::Value::Array(
        predicate_names
            .iter()
            .map(|name| serde_json::Value::String(name.clone()))
            .collect(),
    );
    schema
}
