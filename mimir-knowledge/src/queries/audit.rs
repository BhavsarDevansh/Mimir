//! Audit log queries with filtering and human-readable joins.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::QueryBuilder;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::audit_log::ChangeType;

/// Filter parameters for the audit log query.
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub entity_name: Option<String>,
    pub relationship_type_name: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub change_type: Option<ChangeType>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// A row from the joined audit log query.
///
/// Lookup names that match the enum wire contract (`change_type_name`) are
/// joined from their lookup tables; `changed_by_id` carries the stored
/// discriminant so callers resolve the wire string through `ChangedBy`
/// (issue #380).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AuditLogRow {
    pub audit_id: i32,
    pub fact_id: i32,
    pub entity_name: Option<String>,
    pub relationship_type_name: Option<String>,
    pub change_type_name: String,
    pub changed_by_id: Option<i16>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub changed_at: DateTime<Utc>,
    pub reason: Option<String>,
}

/// Query the audit log with optional filters, ordered by `changed_at ASC`.
pub async fn query_audit_log(
    pool: &SqlitePool,
    filter: &AuditLogFilter,
) -> Result<Vec<AuditLogRow>, KnowledgeError> {
    let mut builder: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT \
            fal.id AS audit_id, \
            fal.fact_id, \
            e.name AS entity_name, \
            rt.name AS relationship_type_name, \
            ct.name AS change_type_name, \
            fal.changed_by_id, \
            fal.old_value, \
            fal.new_value, \
            fal.changed_at, \
            fal.reason \
         FROM fact_audit_log fal \
         LEFT JOIN facts f ON f.id = fal.fact_id \
         LEFT JOIN entities e ON e.id = f.subject_id \
         LEFT JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         JOIN change_types ct ON ct.id = fal.change_type_id \
         WHERE 1=1",
    );

    if let Some(ref name) = filter.entity_name {
        builder.push(" AND e.name = ");
        builder.push_bind(name);
    }
    if let Some(ref name) = filter.relationship_type_name {
        // Resolve through the alias table (the single source of truth for
        // predicate canonicalization) so alias names like `is_in` match facts
        // stored under `located_in` (issue #403).
        let resolved: Option<i16> =
            match crate::normalize_alias(name) {
                Some(normalized) => sqlx::query_scalar(
                    "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
                )
                .bind(normalized)
                .fetch_optional(pool)
                .await?,
                None => None,
            };
        match resolved {
            Some(id) => {
                builder.push(" AND f.relationship_type_id = ");
                builder.push_bind(id);
            }
            None => {
                // Unknown predicate: no facts can match.
                builder.push(" AND 1 = 0");
            }
        }
    }
    if let Some(from) = filter.from {
        builder.push(" AND fal.changed_at >= ");
        builder.push_bind(from);
    }
    if let Some(to) = filter.to {
        builder.push(" AND fal.changed_at <= ");
        builder.push_bind(to);
    }
    if let Some(ct) = filter.change_type {
        builder.push(" AND fal.change_type_id = ");
        builder.push_bind(ct as i16);
    }

    builder.push(" ORDER BY fal.changed_at ASC");

    let limit = filter.limit.unwrap_or(1000);
    builder.push(" LIMIT ");
    builder.push_bind(limit);

    if let Some(offset) = filter.offset {
        builder.push(" OFFSET ");
        builder.push_bind(offset);
    }

    let rows = builder
        .build_query_as::<AuditLogRow>()
        .fetch_all(pool)
        .await?;

    Ok(rows)
}
