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

impl TryFrom<i16> for FactStatus {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Active as i16 => Ok(Self::Active),
            x if x == Self::Inferred as i16 => Ok(Self::Inferred),
            x if x == Self::Disputed as i16 => Ok(Self::Disputed),
            x if x == Self::Corrected as i16 => Ok(Self::Corrected),
            x if x == Self::Superseded as i16 => Ok(Self::Superseded),
            x if x == Self::Forgotten as i16 => Ok(Self::Forgotten),
            _ => Err(()),
        }
    }
}

impl FactStatus {
    /// Wire representation of the fact status.
    ///
    /// The HTTP API (`mimir-api-types`) carries fact statuses as strings, so
    /// this is the single source of truth for the wire contract — independent
    /// of the derived `Debug` repr (issue #293).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Inferred => "Inferred",
            Self::Disputed => "Disputed",
            Self::Corrected => "Corrected",
            Self::Superseded => "Superseded",
            Self::Forgotten => "Forgotten",
        }
    }
}

impl std::str::FromStr for FactStatus {
    type Err = ();

    /// Parse a wire `status` string back into the enum.
    ///
    /// Mirrors [`FactStatus::as_str`] case-insensitively, matching the
    /// historical `kb edit` input contract, so the input and output
    /// directions share one string table (issue #293).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [
            Self::Active,
            Self::Inferred,
            Self::Disputed,
            Self::Corrected,
            Self::Superseded,
            Self::Forgotten,
        ]
        .into_iter()
        .find(|status| status.as_str().eq_ignore_ascii_case(s))
        .ok_or(())
    }
}

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
        FactStatus::try_from(self.fact_status_id).ok()
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

    #[test]
    fn fact_status_try_from_roundtrip() {
        for id in 1..=6 {
            let status = FactStatus::try_from(id).unwrap();
            assert_eq!(status as i16, id);
            assert_eq!(FactStatus::try_from(status as i16), Ok(status));
        }
        assert!(FactStatus::try_from(0).is_err());
        assert!(FactStatus::try_from(7).is_err());
    }

    #[test]
    fn fact_status_as_str_matches_wire_contract() {
        assert_eq!(FactStatus::Active.as_str(), "Active");
        assert_eq!(FactStatus::Inferred.as_str(), "Inferred");
        assert_eq!(FactStatus::Disputed.as_str(), "Disputed");
        assert_eq!(FactStatus::Corrected.as_str(), "Corrected");
        assert_eq!(FactStatus::Superseded.as_str(), "Superseded");
        assert_eq!(FactStatus::Forgotten.as_str(), "Forgotten");
    }

    #[test]
    fn fact_status_from_str_accepts_wire_strings() {
        assert_eq!("active".parse(), Ok(FactStatus::Active));
        assert_eq!("inferred".parse(), Ok(FactStatus::Inferred));
        assert_eq!("disputed".parse(), Ok(FactStatus::Disputed));
        assert_eq!("corrected".parse(), Ok(FactStatus::Corrected));
        assert_eq!("superseded".parse(), Ok(FactStatus::Superseded));
        assert_eq!("forgotten".parse(), Ok(FactStatus::Forgotten));
        // The kb edit endpoint historically accepted any casing.
        assert_eq!("Active".parse(), Ok(FactStatus::Active));
        assert!("bogus".parse::<FactStatus>().is_err());
    }
}
