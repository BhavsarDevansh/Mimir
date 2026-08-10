//! Fact reads: by-id, by-subject, by-predicate, by-object, point-in-time.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::fact::{Fact, FactStatus};
pub async fn get_by_id(pool: &SqlitePool, fact_id: i32) -> Result<Option<Fact>, KnowledgeError> {
    let fact: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(pool)
    .await?;

    Ok(fact)
}

/// List facts for a subject entity.
pub async fn get_by_subject(
    pool: &SqlitePool,
    subject_id: i32,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE subject_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(subject_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

/// List facts for a predicate.
pub async fn get_by_predicate(
    pool: &SqlitePool,
    relationship_type_id: i16,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE relationship_type_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(relationship_type_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

/// List facts for an object entity.
pub async fn get_by_object(
    pool: &SqlitePool,
    object_id: i32,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts WHERE object_id = ? ORDER BY id ASC LIMIT ?",
    )
    .bind(object_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}

/// Return facts active at a specific point in time.
pub async fn get_active_facts_at(
    pool: &SqlitePool,
    subject_id: i32,
    relationship_type_id: i16,
    at: DateTime<Utc>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ? \
           AND fact_status_id = ? \
           AND (valid_from IS NULL OR valid_from <= ?) \
           AND (valid_until IS NULL OR valid_until > ?) \
         ORDER BY valid_from",
    )
    .bind(subject_id)
    .bind(relationship_type_id)
    .bind(FactStatus::Active as i16)
    .bind(at)
    .bind(at)
    .fetch_all(pool)
    .await?;

    Ok(facts)
}
