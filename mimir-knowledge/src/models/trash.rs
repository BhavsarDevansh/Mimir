//! Trash bin model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::models::fact::Fact;
use crate::models::source::Source;

/// A row in the `trash` table.
#[derive(Debug, Clone, PartialEq, FromRow, Serialize, Deserialize)]
pub struct TrashEntry {
    pub id: i32,
    pub original_table: String,
    pub original_id: i32,
    pub payload: String,
    pub deleted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub restored_at: Option<DateTime<Utc>>,
    pub restorer: Option<String>,
}

/// JSON payload stored inside `trash.payload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashPayload {
    pub fact: Fact,
    pub sources: Vec<Source>,
    /// (parent_fact_id, relation_type_id) pairs to rebuild on restore.
    pub dependencies: Vec<(i32, i16)>,
}

/// Summary for listing trash contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashListItem {
    pub trash_id: i32,
    pub fact_id: i32,
    pub subject_name: Option<String>,
    pub predicate_name: Option<String>,
    pub object_name: Option<String>,
    pub object_literal: Option<String>,
    pub deleted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
