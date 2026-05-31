//! Fact model and fact-status enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

use crate::models::enums::Predicate;

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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Fact {
    /// Map the stored `fact_status_id` to the typed enum.
    pub fn status(&self) -> FactStatus {
        match self.fact_status_id {
            1 => FactStatus::Active,
            2 => FactStatus::Inferred,
            3 => FactStatus::Disputed,
            4 => FactStatus::Corrected,
            5 => FactStatus::Superseded,
            6 => FactStatus::Forgotten,
            _ => FactStatus::Active,
        }
    }

    /// Map the stored `predicate_id` to the typed enum.
    pub fn predicate(&self) -> Predicate {
        match self.predicate_id {
            1 => Predicate::IsIn,
            2 => Predicate::Visited,
            3 => Predicate::Owns,
            4 => Predicate::WorksAs,
            5 => Predicate::HasPartner,
            6 => Predicate::HasParent,
            7 => Predicate::BornOn,
            8 => Predicate::DiedOn,
            9 => Predicate::LocatedIn,
            10 => Predicate::CreatedOn,
            _ => Predicate::IsIn,
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
    pub confidence: Option<f32>,
}
