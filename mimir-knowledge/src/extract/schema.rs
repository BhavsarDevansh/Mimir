//! Structured extraction schema: the LLM's `remember` output contract.

use serde::{Deserialize, Serialize};

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
    /// How the fact's date recurs, if at all (events subsystem, #74).
    ///
    /// Emitted by the LLM for recurring facts such as birthdays. One of
    /// `none`, `daily`, `weekly`, `monthly`, `yearly`. Rust validates and maps
    /// it to [`RecurrenceType`](crate::models::enums::RecurrenceType); no natural-language parsing is done in Rust.
    #[serde(default)]
    pub recurrence: Option<String>,
    /// Whether this fact describes a task/deadline that requires the user to
    /// act (stays `Active` past the trigger date instead of auto-completing).
    #[serde(default)]
    pub requires_user_action: Option<bool>,
    /// Optional structured location for a "where" fact (Phase 3 S3 / #193).
    /// When present, the resolved subject entity gets an `entity_locations`
    /// row derived from this overlay and the fact's temporal bounds.
    #[serde(default)]
    pub location: Option<ExtractedLocation>,
}

/// Structured location overlay emitted by the LLM for a "where" fact
/// (Phase 3 S3 / #193). Rust validates and maps it onto
/// [`NormalizedLocation`](crate::normalize::NormalizedLocation); no natural-language parsing happens downstream.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExtractedLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// Wrapper returned by the `remember` tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RememberOutput {
    pub facts: Vec<ExtractedFact>,
}

// ---------------------------------------------------------------------------
// Outcome types (defined in [`crate::normalize`], re-exported for callers
// that still reach them via `mimir_knowledge::extract`).
// ---------------------------------------------------------------------------

pub use crate::normalize::{ExtractionOutcome, PendingFact};
