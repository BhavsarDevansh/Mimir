//! Source model, source-type enum, and extraction-method enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// Origin of a fact in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum SourceType {
    UserEdit = 1,
    Connector = 2,
    Inference = 3,
    Interaction = 4,
    Import = 5,
    System = 6,
}

const_assert!((SourceType::UserEdit as i16) != 0);

/// How a fact was extracted from its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum ExtractionMethod {
    LlmExtraction = 1,
    StructuredParse = 2,
    UserInput = 3,
    InferenceRule = 4,
    DedupMerge = 5,
}

const_assert!((ExtractionMethod::LlmExtraction as i16) != 0);

/// Provenance record linking a fact to its origin.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Source {
    pub id: i32,
    pub fact_id: i32,
    pub source_type_id: i16,
    pub connector_id: Option<String>,
    pub connector_type_id: Option<i16>,
    pub raw_reference: Option<String>,
    pub extracted_at: DateTime<Utc>,
    pub extraction_method_id: Option<i16>,
}
