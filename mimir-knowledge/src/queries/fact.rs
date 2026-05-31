//! Fact CRUD, temporal queries, overlap logic, and audit logging.

use chrono::{DateTime, Utc};
use serde_json;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::fact::{Fact, FactStatus, NewFact};

// ---------------------------------------------------------------------------
// Insert
// ---------------------------------------------------------------------------

/// Insert a new fact with transactional provenance and temporal overlap handling.
///
/// Temporal rules (same `subject_id + predicate_id`):
/// - Non-overlapping ranges → `Active`
/// - Overlapping with both unbounded → `Disputed`
/// - Old open-ended + new explicit starting now → close old at `now()`, new `Active`
/// - Any other overlap → `Disputed`
pub async fn insert_fact(
    pool: &SqlitePool,
    new_fact: &NewFact,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;

    // 1. Temporal overlap check against same subject + predicate.
    let existing: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND predicate_id = ?",
    )
    .bind(new_fact.subject_id)
    .bind(new_fact.predicate as i16)
    .fetch_all(&mut *tx)
    .await?;

    let mut fact_status = FactStatus::Active;
    for existing_fact in &existing {
        if ranges_overlap(
            existing_fact.valid_from,
            existing_fact.valid_until,
            new_fact.valid_from,
            new_fact.valid_until,
        ) {
            if existing_fact.valid_until.is_none() && new_fact.valid_until.is_none() {
                fact_status = FactStatus::Disputed;
                break;
            }
            if existing_fact.valid_until.is_none() && new_fact.valid_from.is_some() {
                sqlx::query("UPDATE facts SET valid_until = ?, updated_at = ? WHERE id = ?")
                    .bind(now)
                    .bind(now)
                    .bind(existing_fact.id)
                    .execute(&mut *tx)
                    .await?;
                // Continue checking other overlaps.
            } else {
                fact_status = FactStatus::Disputed;
                break;
            }
        }
    }

    // 2. Compute confidence (placeholder — extracted to confidence module in #51).
    let confidence = new_fact
        .confidence
        .unwrap_or_else(|| crate::confidence::initial(new_fact.source_type));

    // 3. Insert fact.
    let fact: Fact = sqlx::query_as::<_, Fact>(
        "INSERT INTO facts (subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at",
    )
    .bind(new_fact.subject_id)
    .bind(new_fact.predicate as i16)
    .bind(new_fact.object_id)
    .bind(&new_fact.object_literal)
    .bind(new_fact.valid_from)
    .bind(new_fact.valid_until)
    .bind(confidence)
    .bind(fact_status as i16)
    .bind(false) // inferred
    .fetch_one(&mut *tx)
    .await?;

    // 4. Insert source.
    sqlx::query("INSERT INTO sources (fact_id, source_type_id) VALUES (?, ?)")
        .bind(fact.id)
        .bind(new_fact.source_type as i16)
        .execute(&mut *tx)
        .await?;

    // 5. Audit log.
    let new_json = serde_json::to_string(&fact).unwrap_or_default();
    sqlx::query(
        "INSERT INTO fact_audit_log (fact_id, action, new_value, performed_at, performer) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(fact.id)
    .bind("INSERT")
    .bind(new_json)
    .bind(now)
    .bind("system")
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(fact)
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Retrieve a single fact by primary key.
pub async fn get_by_id(pool: &SqlitePool, id: i32) -> Result<Option<Fact>, KnowledgeError> {
    let fact: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(fact)
}

/// List facts for a given subject, newest first.
pub async fn get_by_subject(
    pool: &SqlitePool,
    subject_id: i32,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? \
         ORDER BY created_at DESC \
         LIMIT ?",
    )
    .bind(subject_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(facts)
}

/// List facts for a given predicate, newest first.
pub async fn get_by_predicate(
    pool: &SqlitePool,
    predicate_id: i16,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts \
         WHERE predicate_id = ? \
         ORDER BY created_at DESC \
         LIMIT ?",
    )
    .bind(predicate_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(facts)
}

/// List facts for a given object entity, newest first.
pub async fn get_by_object(
    pool: &SqlitePool,
    object_id: i32,
    limit: i64,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts \
         WHERE object_id = ? \
         ORDER BY created_at DESC \
         LIMIT ?",
    )
    .bind(object_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(facts)
}

/// Return facts that are active at a specific point in time.
///
/// A fact is active at `at_time` when:
/// - `valid_from IS NULL OR valid_from <= at_time`
/// - `valid_until IS NULL OR valid_until >= at_time`
/// - `fact_status_id = 1` (Active)
pub async fn get_active_facts_at(
    pool: &SqlitePool,
    subject_id: i32,
    predicate_id: i16,
    at_time: DateTime<Utc>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND predicate_id = ? \
         AND (valid_from IS NULL OR valid_from <= ?) \
         AND (valid_until IS NULL OR valid_until >= ?) \
         AND fact_status_id = 1",
    )
    .bind(subject_id)
    .bind(predicate_id)
    .bind(at_time)
    .bind(at_time)
    .fetch_all(pool)
    .await?;
    Ok(facts)
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Update the `valid_until` timestamp of a fact.
///
/// Rejects updates to immutable fields by only touching `valid_until`.
pub async fn update_valid_until(
    pool: &SqlitePool,
    fact_id: i32,
    new_valid_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;

    let old = old.ok_or(KnowledgeError::FactNotFound(fact_id))?;
    let old_json = serde_json::to_string(&old).unwrap_or_default();

    sqlx::query("UPDATE facts SET valid_until = ?, updated_at = ? WHERE id = ?")
        .bind(new_valid_until)
        .bind(now)
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;

    let updated: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_json = serde_json::to_string(&updated).unwrap_or_default();
    sqlx::query(
        "INSERT INTO fact_audit_log (fact_id, action, old_value, new_value, performed_at, performer) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind("UPDATE")
    .bind(old_json)
    .bind(new_json)
    .bind(now)
    .bind("system")
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(updated)
}

/// Update the lifecycle status of a fact, writing an audit log entry.
pub async fn set_status(
    pool: &SqlitePool,
    fact_id: i32,
    new_status: FactStatus,
    now: DateTime<Utc>,
) -> Result<Fact, KnowledgeError> {
    let mut tx = pool.begin().await?;

    let old: Option<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;

    let old = old.ok_or(KnowledgeError::FactNotFound(fact_id))?;
    let old_json = serde_json::to_string(&old).unwrap_or_default();

    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
        .bind(new_status as i16)
        .bind(now)
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;

    let updated: Fact = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, predicate_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         created_at, updated_at \
         FROM facts WHERE id = ?",
    )
    .bind(fact_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_json = serde_json::to_string(&updated).unwrap_or_default();
    sqlx::query(
        "INSERT INTO fact_audit_log (fact_id, action, old_value, new_value, performed_at, performer) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(fact_id)
    .bind("STATUS_CHANGE")
    .bind(old_json)
    .bind(new_json)
    .bind(now)
    .bind("system")
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Determine whether two optional time ranges overlap.
///
/// A range is `[from, until)` where `None` means unbounded on that side.
fn ranges_overlap(
    a_from: Option<DateTime<Utc>>,
    a_until: Option<DateTime<Utc>>,
    b_from: Option<DateTime<Utc>>,
    b_until: Option<DateTime<Utc>>,
) -> bool {
    let a_starts_before_b_ends = match (a_from, b_until) {
        (None, _) => true,
        (_, None) => true,
        (Some(af), Some(bu)) => af < bu,
    };
    let b_starts_before_a_ends = match (b_from, a_until) {
        (None, _) => true,
        (_, None) => true,
        (Some(bf), Some(au)) => bf < au,
    };
    a_starts_before_b_ends && b_starts_before_a_ends
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

/// Retrieve audit log entries for a given fact, newest first.
pub async fn get_audit_log(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Vec<crate::models::audit_log::AuditLogEntry>, KnowledgeError> {
    let entries: Vec<crate::models::audit_log::AuditLogEntry> =
        sqlx::query_as::<_, crate::models::audit_log::AuditLogEntry>(
            "SELECT id, fact_id, action, old_value, new_value, performed_at, performer \
         FROM fact_audit_log \
         WHERE fact_id = ? \
         ORDER BY performed_at DESC",
        )
        .bind(fact_id)
        .fetch_all(pool)
        .await?;
    Ok(entries)
}
