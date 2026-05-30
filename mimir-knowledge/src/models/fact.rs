//! Fact model and fact-status enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

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
    pub predicate: String,
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
