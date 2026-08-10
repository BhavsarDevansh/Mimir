//! Pending-confirmation fact listing.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;

/// A pending sensitive fact with resolved subject, predicate, and object names.
///
/// Used by the confirmation lifecycle surface (`GET /kb/pending`).
#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
pub struct PendingFactRow {
    pub fact_id: i32,
    pub subject: String,
    pub predicate: String,
    /// Resolved object: entity name when the object is an entity, else the
    /// literal value.
    pub object: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// List all facts awaiting user confirmation (`pending_confirmation = TRUE`),
/// oldest first, joined to resolve human-readable subject, predicate, and
/// object names.
pub async fn list_pending(pool: &SqlitePool) -> Result<Vec<PendingFactRow>, KnowledgeError> {
    let rows: Vec<PendingFactRow> = sqlx::query_as::<_, PendingFactRow>(
        "SELECT f.id AS fact_id, \
                s.name AS subject, \
                rt.name AS predicate, \
                COALESCE(o.name, f.object_literal) AS object, \
                f.created_at AS created_at \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         WHERE f.pending_confirmation = TRUE \
         ORDER BY f.created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
