//! Memory schema and ranking types for the condensation pipeline.

use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// Priority tier assigned to a fact for memory inclusion ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum MemoryPriority {
    Critical = 1,
    High = 2,
    Normal = 3,
    Low = 4,
}

const_assert!((MemoryPriority::Critical as i16) != 0);

impl MemoryPriority {
    /// Multiplicative boost applied to a fact's score based on priority.
    pub fn boost(self) -> f32 {
        match self {
            MemoryPriority::Critical => 2.0,
            MemoryPriority::High => 1.5,
            MemoryPriority::Normal => 1.0,
            MemoryPriority::Low => 0.5,
        }
    }
}

/// A bucket in the structured memory schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryBucket {
    Identity,
    Relationships,
    Preferences,
    Upcoming,
    General,
}

/// A fact enriched with ranking metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedFact {
    pub fact_id: i32,
    pub subject_name: String,
    pub relationship_type: String,
    pub object_display: String,
    pub confidence: f32,
    pub score: f32,
    pub temporal_boost: f32,
    pub memory_weight: f32,
    pub priority_boost: f32,
    pub centrality_boost: f32,
    pub category_ids: Vec<i32>,
    pub bucket: MemoryBucket,
    pub char_estimate: usize,
}

/// The fully structured memory output consumed by the renderer or LLM condensation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySchema {
    pub identity: Vec<RankedFact>,
    pub relationships: Vec<RankedFact>,
    pub preferences: Vec<RankedFact>,
    pub upcoming: Vec<RankedFact>,
    pub general: Vec<RankedFact>,
    pub total_score: f32,
    pub char_count: usize,
}

impl MemorySchema {
    pub fn new() -> Self {
        Self {
            identity: Vec::new(),
            relationships: Vec::new(),
            preferences: Vec::new(),
            upcoming: Vec::new(),
            general: Vec::new(),
            total_score: 0.0,
            char_count: 0,
        }
    }

    /// All facts across all buckets in display order.
    pub fn all_facts(&self) -> Vec<&RankedFact> {
        let mut out = Vec::new();
        out.extend(&self.identity);
        out.extend(&self.relationships);
        out.extend(&self.preferences);
        out.extend(&self.upcoming);
        out.extend(&self.general);
        out
    }
}

impl Default for MemorySchema {
    fn default() -> Self {
        Self::new()
    }
}
