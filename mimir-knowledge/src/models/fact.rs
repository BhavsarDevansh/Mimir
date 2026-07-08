//! Fact model and fact-status enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

use crate::models::enums::ConnectorType;

/// Lifecycle status of a fact in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum FactStatus {
    Active = 1,
    Inferred = 2,
    Disputed = 3,
    Corrected = 4,
    Superseded = 5,
    Forgotten = 6,
}

const_assert!((FactStatus::Active as i16) != 0);

fn default_memory_priority_id() -> i16 {
    3 // Normal
}

/// A directed, temporal edge between entities (or a literal value).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Fact {
    pub id: i32,
    pub subject_id: i32,
    pub relationship_type_id: i16,
    pub object_id: Option<i32>,
    pub object_literal: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub fact_status_id: i16,
    pub inferred: bool,
    pub inference_depth: i32,
    pub stale_confidence: bool,
    #[serde(default = "default_memory_priority_id")]
    pub memory_priority_id: i16,
    pub created_at: DateTime<Utc>,
    pub pending_confirmation: bool,
    pub updated_at: DateTime<Utc>,
}

impl Fact {
    /// Map the stored `fact_status_id` to the typed enum.
    /// Returns `None` if the ID does not correspond to a known variant.
    pub fn status(&self) -> Option<FactStatus> {
        match self.fact_status_id {
            1 => Some(FactStatus::Active),
            2 => Some(FactStatus::Inferred),
            3 => Some(FactStatus::Disputed),
            4 => Some(FactStatus::Corrected),
            5 => Some(FactStatus::Superseded),
            6 => Some(FactStatus::Forgotten),
            _ => None,
        }
    }
}

/// Input for inserting a new fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewFact {
    pub subject_id: i32,
    pub relationship_type: String,
    pub object_id: Option<i32>,
    pub object_literal: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub source_type: crate::models::source::SourceType,
    pub connector_instance_id: Option<i32>,
    pub connector_type: Option<ConnectorType>,
    pub raw_reference: Option<String>,
    pub extraction_method: Option<crate::models::source::ExtractionMethod>,
    pub inferred: bool,
    pub inference_depth: i32,
    pub confidence: Option<f32>,
    pub parent_fact_ids: Vec<i32>,
    pub category_ids: Vec<i32>,
}

impl NewFact {
    pub fn new(subject_id: i32, relationship_type: impl Into<String>) -> Self {
        Self {
            subject_id,
            relationship_type: relationship_type.into(),
            object_id: None,
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: crate::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_status_roundtrip() {
        assert_eq!(FactStatus::Active as i16, 1);
        assert_eq!(FactStatus::Inferred as i16, 2);
        assert_eq!(FactStatus::Disputed as i16, 3);
        assert_eq!(FactStatus::Corrected as i16, 4);
        assert_eq!(FactStatus::Superseded as i16, 5);
        assert_eq!(FactStatus::Forgotten as i16, 6);
    }

    #[test]
    fn fact_status_method_maps_correctly() {
        let mut fact = Fact {
            id: 1,
            subject_id: 1,
            relationship_type_id: 1,
            object_id: None,
            object_literal: None,
            valid_from: None,
            valid_until: None,
            confidence: 1.0,
            fact_status_id: FactStatus::Active as i16,
            inferred: false,
            inference_depth: 0,
            stale_confidence: false,
            memory_priority_id: 3,
            created_at: Utc::now(),
            pending_confirmation: false,
            updated_at: Utc::now(),
        };
        assert_eq!(fact.status(), Some(FactStatus::Active));

        fact.fact_status_id = 99;
        assert_eq!(fact.status(), None);
    }

    #[test]
    fn new_fact_defaults() {
        let nf = NewFact::new(7, "likes");
        assert_eq!(nf.subject_id, 7);
        assert_eq!(nf.relationship_type, "likes");
        assert_eq!(nf.object_id, None);
        assert!(!nf.inferred);
        assert_eq!(nf.inference_depth, 0);
        assert_eq!(nf.confidence, None);
        assert!(nf.parent_fact_ids.is_empty());
        assert!(nf.category_ids.is_empty());
    }
}
