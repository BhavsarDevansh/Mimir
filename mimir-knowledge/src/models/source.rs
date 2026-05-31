//! Source model and source-type enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// Origin of a fact in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum SourceType {
    Email = 1,
    Calendar = 2,
    Photo = 3,
    Message = 4,
    Inference = 5,
    UserEdit = 6,
    Connector = 7,
    CasualMention = 8,
    Import = 9,
    System = 10,
}

const_assert!((SourceType::Email as i16) != 0);

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
    pub extraction_method: Option<String>,
}
