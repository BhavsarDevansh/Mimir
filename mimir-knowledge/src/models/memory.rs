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
///
/// Discriminants match the `memory_buckets` lookup rows seeded by migration
/// `052` and encode the classification priority: the memory query resolves a
/// multi-category fact to the bucket with the lowest id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[repr(i16)]
pub enum MemoryBucket {
    /// Identity facts always rank first in the schema.
    Identity = 1,
    /// Facts about future-dated events and recurring dates.
    Upcoming = 2,
    /// Facts about people and social ties.
    Relationships = 3,
    /// Facts about the user's preferences and habits.
    Preferences = 4,
    /// Everything that does not fit a more specific bucket.
    General = 5,
}

impl TryFrom<i16> for MemoryBucket {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Identity as i16 => Ok(Self::Identity),
            x if x == Self::Upcoming as i16 => Ok(Self::Upcoming),
            x if x == Self::Relationships as i16 => Ok(Self::Relationships),
            x if x == Self::Preferences as i16 => Ok(Self::Preferences),
            x if x == Self::General as i16 => Ok(Self::General),
            _ => Err(()),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_values_are_ordered() {
        assert!(MemoryPriority::Critical.boost() > MemoryPriority::High.boost());
        assert!(MemoryPriority::High.boost() > MemoryPriority::Normal.boost());
        assert!(MemoryPriority::Normal.boost() > MemoryPriority::Low.boost());
    }

    #[test]
    fn boost_exact_values() {
        assert_eq!(MemoryPriority::Critical.boost(), 2.0);
        assert_eq!(MemoryPriority::High.boost(), 1.5);
        assert_eq!(MemoryPriority::Normal.boost(), 1.0);
        assert_eq!(MemoryPriority::Low.boost(), 0.5);
    }

    #[test]
    fn memory_priority_discriminant_nonzero_and_stable() {
        assert_eq!(MemoryPriority::Critical as i16, 1);
        assert_eq!(MemoryPriority::High as i16, 2);
        assert_eq!(MemoryPriority::Normal as i16, 3);
        assert_eq!(MemoryPriority::Low as i16, 4);
    }

    #[test]
    fn new_schema_is_empty_with_zero_scores() {
        let schema = MemorySchema::new();
        assert!(schema.identity.is_empty());
        assert!(schema.relationships.is_empty());
        assert!(schema.preferences.is_empty());
        assert!(schema.upcoming.is_empty());
        assert!(schema.general.is_empty());
        assert_eq!(schema.total_score, 0.0);
        assert_eq!(schema.char_count, 0);
        assert!(schema.all_facts().is_empty());
    }

    #[test]
    fn default_equals_new() {
        assert_eq!(MemorySchema::default(), MemorySchema::new());
    }

    #[test]
    fn all_facts_preserves_display_order() {
        let mk = |id: i32, bucket: MemoryBucket| RankedFact {
            fact_id: id,
            subject_name: format!("s{id}"),
            relationship_type: "r".to_string(),
            object_display: "o".to_string(),
            confidence: 1.0,
            score: 1.0,
            temporal_boost: 0.0,
            memory_weight: 1.0,
            priority_boost: 1.0,
            centrality_boost: 0.0,
            category_ids: vec![],
            bucket,
            char_estimate: 10,
        };
        let schema = MemorySchema {
            identity: vec![mk(1, MemoryBucket::Identity)],
            relationships: vec![
                mk(2, MemoryBucket::Relationships),
                mk(3, MemoryBucket::Relationships),
            ],
            preferences: vec![mk(4, MemoryBucket::Preferences)],
            upcoming: vec![],
            general: vec![mk(5, MemoryBucket::General)],
            total_score: 5.0,
            char_count: 50,
        };
        let all = schema.all_facts();
        assert_eq!(
            all.iter().map(|f| f.fact_id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn memory_schema_serde_roundtrip() {
        let schema = MemorySchema::new();
        let json = serde_json::to_string(&schema).unwrap();
        let back: MemorySchema = serde_json::from_str(&json).unwrap();
        assert_eq!(back, schema);
    }
}
