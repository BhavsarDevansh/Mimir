//! Audit log model for fact lifecycle events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single entry in the `fact_audit_log` table.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i32,
    pub fact_id: i32,
    pub action: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub performed_at: DateTime<Utc>,
    pub performer: Option<String>,
}
