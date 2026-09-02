//! Memory ranking, selection, budget fill, and deterministic fallback rendering.

use chrono::{DateTime, Utc};

use crate::models::memory::MemoryBucket;

mod build;
mod ranking;
mod render;
#[cfg(test)]
mod tests;

pub use build::build_memory_schema;
pub use build::build_memory_schema_with_opts;
pub use render::{refresh_now_line, render_memory_schema, render_upcoming_section};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildMemoryOptions {
    /// Buckets whose facts are collected but do not consume the character budget.
    pub exclude_from_budget: Vec<MemoryBucket>,
    /// If true, exclude facts whose relationship type is marked sensitive.
    pub exclude_sensitive: bool,
}

// ---------------------------------------------------------------------------
// Raw row from the enriched fact query
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct RawRankedFact {
    fact_id: i32,
    subject_name: String,
    relationship_type: String,
    object_name: Option<String>,
    object_literal: Option<String>,
    confidence: f32,
    valid_from: Option<DateTime<Utc>>,
    category_ids: Option<String>, // comma-separated
    memory_weight: Option<f32>,
    memory_bucket_id: Option<i16>,
    memory_priority_id: i16,
}
