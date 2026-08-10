//! Enriched fact browsing: audit log, subject-filtered lists, relationship subtrees.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::source::Source;
pub async fn get_audit_log(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Vec<crate::models::audit_log::AuditLogEntry>, KnowledgeError> {
    let entries: Vec<crate::models::audit_log::AuditLogEntry> =
        sqlx::query_as::<_, crate::models::audit_log::AuditLogEntry>(
            "SELECT id, fact_id, change_type_id, old_value, new_value, \
             changed_at, changed_by_id, reason \
             FROM fact_audit_log \
             WHERE fact_id = ? \
             ORDER BY changed_at DESC",
        )
        .bind(fact_id)
        .fetch_all(pool)
        .await?;
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Filtered fact queries for tool layer
// ---------------------------------------------------------------------------

/// A fact row joined with the object entity name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FactWithObjectName {
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
    pub pending_confirmation: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub object_name: Option<String>,
}

/// A fact enriched with its object name and source records.
#[derive(Debug, Clone)]
pub struct FactWithSources {
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
    pub pending_confirmation: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub object_name: Option<String>,
    pub sources: Vec<Source>,
}

/// Batch-fetch sources for the given fact rows and assemble enriched
/// `FactWithSources` records (object name + sources), preserving row order.
async fn enrich_with_sources(
    pool: &SqlitePool,
    rows: Vec<FactWithObjectName>,
) -> Result<Vec<FactWithSources>, KnowledgeError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let fact_ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    let placeholders: Vec<&str> = fact_ids.iter().map(|_| "?").collect();
    let src_sql = format!(
        "SELECT id, fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference, extracted_at, extraction_method_id \
         FROM sources \
         WHERE fact_id IN ({})",
        placeholders.join(",")
    );
    let mut src_query = sqlx::query_as::<_, Source>(sqlx::AssertSqlSafe(&*src_sql));
    for &id in &fact_ids {
        src_query = src_query.bind(id);
    }
    let sources = src_query.fetch_all(pool).await?;

    let mut sources_by_fact: std::collections::HashMap<i32, Vec<Source>> =
        std::collections::HashMap::new();
    for src in sources {
        sources_by_fact.entry(src.fact_id).or_default().push(src);
    }

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let srcs = sources_by_fact.remove(&row.id).unwrap_or_default();
        results.push(FactWithSources {
            id: row.id,
            subject_id: row.subject_id,
            relationship_type_id: row.relationship_type_id,
            object_id: row.object_id,
            object_literal: row.object_literal,
            valid_from: row.valid_from,
            valid_until: row.valid_until,
            confidence: row.confidence,
            fact_status_id: row.fact_status_id,
            inferred: row.inferred,
            inference_depth: row.inference_depth,
            stale_confidence: row.stale_confidence,
            pending_confirmation: row.pending_confirmation,
            created_at: row.created_at,
            updated_at: row.updated_at,
            object_name: row.object_name,
            sources: srcs,
        });
    }

    Ok(results)
}

/// Retrieve facts for a subject with optional predicate filter and confidence threshold.
pub async fn get_facts_by_subject_filtered(
    pool: &SqlitePool,
    subject_id: i32,
    relationship_type_id_opt: Option<i16>,
    min_confidence: f32,
    offset: i64,
    limit: i64,
) -> Result<Vec<FactWithSources>, KnowledgeError> {
    let rows: Vec<FactWithObjectName> = if let Some(relationship_type_id) = relationship_type_id_opt
    {
        sqlx::query_as::<_, FactWithObjectName>(
            "SELECT f.id, f.subject_id, f.relationship_type_id, f.object_id, f.object_literal, \
                    f.valid_from, f.valid_until, f.confidence, f.fact_status_id, f.inferred, \
                    f.inference_depth, f.stale_confidence, f.pending_confirmation, f.created_at, f.updated_at, \
                    e.name as object_name \
             FROM facts f \
             LEFT JOIN entities e ON e.id = f.object_id \
             WHERE f.subject_id = ? \
               AND f.pending_confirmation = 0 \
               AND f.fact_status_id NOT IN (5, 6) \
               AND f.relationship_type_id = ? \
               AND f.confidence >= ? \
             ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC \
             LIMIT ? OFFSET ?",
        )
        .bind(subject_id)
        .bind(relationship_type_id)
        .bind(min_confidence)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, FactWithObjectName>(
            "SELECT f.id, f.subject_id, f.relationship_type_id, f.object_id, f.object_literal, \
                    f.valid_from, f.valid_until, f.confidence, f.fact_status_id, f.inferred, \
                    f.inference_depth, f.stale_confidence, f.pending_confirmation, f.created_at, f.updated_at, \
                    e.name as object_name \
             FROM facts f \
             LEFT JOIN entities e ON e.id = f.object_id \
             WHERE f.subject_id = ? \
               AND f.pending_confirmation = 0 \
               AND f.fact_status_id NOT IN (5, 6) \
               AND f.confidence >= ? \
             ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC \
             LIMIT ? OFFSET ?",
        )
        .bind(subject_id)
        .bind(min_confidence)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };

    enrich_with_sources(pool, rows).await
}

/// Count facts for a subject with optional predicate filter and confidence threshold.
pub async fn count_facts_by_subject_filtered(
    pool: &SqlitePool,
    subject_id: i32,
    relationship_type_id_opt: Option<i16>,
    min_confidence: f32,
) -> Result<i64, KnowledgeError> {
    let count: i64 = if let Some(relationship_type_id) = relationship_type_id_opt {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM facts \
             WHERE subject_id = ? \
               AND pending_confirmation = 0 \
               AND fact_status_id NOT IN (5, 6) \
               AND relationship_type_id = ? \
               AND confidence >= ?",
        )
        .bind(subject_id)
        .bind(relationship_type_id)
        .bind(min_confidence)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM facts \
             WHERE subject_id = ? \
               AND pending_confirmation = 0 \
               AND fact_status_id NOT IN (5, 6) \
               AND confidence >= ?",
        )
        .bind(subject_id)
        .bind(min_confidence)
        .fetch_one(pool)
        .await?
    };
    Ok(count)
}

/// Recursive CTE yielding a relationship type and all of its descendants in the
/// `relationship_type_hierarchy` DAG. The first bound parameter is the root type
/// id; `UNION` (not `UNION ALL`) deduplicates ids reachable via multiple paths.
const RELATIONSHIP_SUBTREE_CTE: &str = "WITH RECURSIVE subtree(id) AS ( \
    SELECT ? \
    UNION \
    SELECT h.child_id FROM relationship_type_hierarchy h \
    JOIN subtree s ON h.parent_id = s.id \
)";

/// Retrieve facts for a subject whose relationship type is `root_type_id` or
/// any descendant in the `relationship_type_hierarchy` DAG.
///
/// Walks the DAG via a single SQLite recursive CTE that seeds with the root
/// type itself, then unions all children. Filters to non-pending facts whose
/// status is not Superseded or Forgotten (`NOT IN (5, 6)`), with confidence at
/// least `min_confidence`, sorted by confidence descending (then `valid_from`
/// descending, then `id` descending). Enriched with the object entity name and
/// batched source records via `enrich_with_sources`.
pub async fn get_facts_by_relationship_subtree(
    pool: &SqlitePool,
    subject_id: i32,
    root_type_id: i16,
    min_confidence: f32,
    limit: i64,
) -> Result<Vec<FactWithSources>, KnowledgeError> {
    let sql = format!(
        "{RELATIONSHIP_SUBTREE_CTE} \
         SELECT f.id, f.subject_id, f.relationship_type_id, f.object_id, f.object_literal, \
                f.valid_from, f.valid_until, f.confidence, f.fact_status_id, f.inferred, \
                f.inference_depth, f.stale_confidence, f.pending_confirmation, f.created_at, f.updated_at, \
                e.name as object_name \
         FROM facts f \
         JOIN subtree s ON f.relationship_type_id = s.id \
         LEFT JOIN entities e ON e.id = f.object_id \
         WHERE f.subject_id = ? \
           AND f.pending_confirmation = 0 \
           AND f.fact_status_id NOT IN (5, 6) \
           AND f.confidence >= ? \
         ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC \
         LIMIT ?"
    );
    let rows: Vec<FactWithObjectName> =
        sqlx::query_as::<_, FactWithObjectName>(sqlx::AssertSqlSafe(&*sql))
            .bind(root_type_id)
            .bind(subject_id)
            .bind(min_confidence)
            .bind(limit)
            .fetch_all(pool)
            .await?;

    enrich_with_sources(pool, rows).await
}

/// Count facts for a subject whose relationship type is `root_type_id` or any
/// descendant, applying the same filters as
/// [`get_facts_by_relationship_subtree`] (non-pending, status `NOT IN (5, 6)`,
/// confidence at least `min_confidence`).
pub async fn count_facts_by_relationship_subtree(
    pool: &SqlitePool,
    subject_id: i32,
    root_type_id: i16,
    min_confidence: f32,
) -> Result<i64, KnowledgeError> {
    let sql = format!(
        "{RELATIONSHIP_SUBTREE_CTE} \
         SELECT COUNT(*) \
         FROM facts f \
         JOIN subtree s ON f.relationship_type_id = s.id \
         WHERE f.subject_id = ? \
           AND f.pending_confirmation = 0 \
           AND f.fact_status_id NOT IN (5, 6) \
           AND f.confidence >= ?"
    );
    let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(&*sql))
        .bind(root_type_id)
        .bind(subject_id)
        .bind(min_confidence)
        .fetch_one(pool)
        .await?;
    Ok(count)
}
