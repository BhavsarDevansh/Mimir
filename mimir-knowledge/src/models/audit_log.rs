//! Audit log model with typed change_type and changed_by enums.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// What happened to a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum ChangeType {
    Created = 1,
    StatusChange = 2,
    ConfidenceChange = 3,
    TemporalUpdate = 4,
    SourceAdded = 5,
    Forgotten = 6,
    Restored = 7,
    Rejected = 8,
}

const_assert!((ChangeType::Created as i16) != 0);

/// Who or what triggered the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum ChangedBy {
    User = 1,
    System = 2,
    InferenceEngine = 3,
    NightlyOptimization = 4,
}

const_assert!((ChangedBy::User as i16) != 0);

/// A single entry in the `fact_audit_log` table.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i32,
    pub fact_id: i32,
    pub change_type_id: i16,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub changed_at: DateTime<Utc>,
    pub changed_by_id: Option<i16>,
    pub reason: Option<String>,
}
