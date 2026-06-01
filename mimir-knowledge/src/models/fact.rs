//! Fact model and fact-status enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

use crate::models::enums::{ConnectorType, Predicate};

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

/// A directed, temporal edge between entities (or a literal value).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Fact {
    pub id: i32,
    pub subject_id: i32,
    pub predicate_id: i16,
    pub object_id: Option<i32>,
    pub object_literal: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub fact_status_id: i16,
    pub inferred: bool,
    pub inference_depth: i32,
    pub stale_confidence: bool,
    pub created_at: DateTime<Utc>,
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

    /// Map the stored `predicate_id` to the typed enum.
    /// Returns `None` if the ID does not correspond to a known variant.
    pub fn predicate(&self) -> Option<Predicate> {
        match self.predicate_id {
            1 => Some(Predicate::IsIn),
            2 => Some(Predicate::Visited),
            3 => Some(Predicate::Owns),
            4 => Some(Predicate::WorksAs),
            5 => Some(Predicate::HasPartner),
            6 => Some(Predicate::HasParent),
            7 => Some(Predicate::BornOn),
            8 => Some(Predicate::DiedOn),
            9 => Some(Predicate::LocatedIn),
            10 => Some(Predicate::CreatedOn),
            _ => None,
        }
    }
}

/// Input for inserting a new fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewFact {
    pub subject_id: i32,
    pub predicate: Predicate,
    pub object_id: Option<i32>,
    pub object_literal: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub source_type: crate::models::source::SourceType,
    pub connector_id: Option<String>,
    pub connector_type: Option<ConnectorType>,
    pub raw_reference: Option<String>,
    pub extraction_method: Option<crate::models::source::ExtractionMethod>,
}
