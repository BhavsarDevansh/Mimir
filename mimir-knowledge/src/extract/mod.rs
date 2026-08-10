//! Conversational fact extraction: LLM `remember` tool → structured
//! [`ExtractedFact`]s → the shared [`crate::normalize::normalize_and_insert`]
//! boundary.
//!
//! Module layout by concern:
//!
//! - `schema` — the extraction schema types ([`Classification`],
//!   [`ExtractedFact`], [`RememberOutput`]).
//! - `tool` — the `remember` tool JSON Schema.
//! - `prompt` — conversation-aware extraction prompts.
//! - `parse` — LLM-output parsing and fact conversion.
//! - `pipeline` — the extraction pipeline entrypoints.
//! - `confirm` — sensitive-fact confirmation / rejection.

mod confirm;
mod parse;
mod pipeline;
mod prompt;
mod schema;
mod tool;

pub use confirm::{confirm_fact, reject_fact};
pub use parse::{
    parse_entity_type, parse_event_type, parse_location, parse_location_type, parse_recurrence,
    parse_temporal_bound,
};
pub use pipeline::{extract_facts, extract_facts_with_context, process_remember_output};
pub use schema::{
    Classification, ExtractedFact, ExtractedLocation, ExtractionOutcome, PendingFact,
    RememberOutput, Temporal,
};
pub use tool::{remember_tool_params_schema, remember_tool_schema};
