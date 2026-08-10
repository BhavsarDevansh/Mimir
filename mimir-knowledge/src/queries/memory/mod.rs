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
pub use render::{render_memory_schema, render_upcoming_section};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildMemoryOptions {
    /// Buckets whose facts are collected but do not consume the character budget.
    pub exclude_from_budget: Vec<MemoryBucket>,
    /// If true, exclude facts whose relationship type is marked sensitive.
    pub exclude_sensitive: bool,
}

// ---------------------------------------------------------------------------
// Bucket category ID constants
// ---------------------------------------------------------------------------
const IDENTITY_CATEGORY_RANGE: std::ops::RangeInclusive<i32> = 100..=199;
const UPCOMING_CATEGORY_RANGE: std::ops::RangeInclusive<i32> = 900..=999;
const RELATIONSHIP_CATEGORY_RANGE: std::ops::RangeInclusive<i32> = 400..=499;
/// Core preference category range (300-399).
const PREFERENCE_CATEGORY_RANGE: std::ops::RangeInclusive<i32> = 300..=399;
/// Outlier preference category IDs outside the main 300-399 range.
const PREFERENCE_CATEGORY_EXTRAS: &[i32] = &[460, 480, 570, 670, 680, 690, 830, 870];

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
    memory_priority_id: i16,
}
