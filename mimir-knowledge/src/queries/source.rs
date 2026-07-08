//! Source CRUD and provenance queries.

use chrono::{DateTime, Utc};
use serde_json;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::source::Source;

/// Input for inserting or updating a source row (internal).
pub struct SourceInput {
    pub fact_id: i32,
    pub source_type_id: i16,
    pub connector_instance_id: Option<i32>,
    pub connector_type_id: Option<i16>,
    pub raw_reference: Option<String>,
    pub extraction_method_id: Option<i16>,
}

/// Public request for adding a source to a fact.
pub struct AddSourceRequest {
    pub fact_id: i32,
    pub source_type: crate::models::source::SourceType,
    pub connector_instance_id: Option<i32>,
    pub connector_type: Option<crate::models::enums::ConnectorType>,
    pub raw_reference: Option<String>,
    pub extraction_method: Option<crate::models::source::ExtractionMethod>,
    pub changed_by: crate::models::audit_log::ChangedBy,
}

/// Insert a source row for a fact.
pub async fn insert_source(
    pool: &SqlitePool,
    input: &SourceInput,
    extracted_at: DateTime<Utc>,
) -> Result<Source, KnowledgeError> {
    let raw_reference_norm = input.raw_reference.as_deref().unwrap_or("");

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sources \
         (fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference, \
         extracted_at, extraction_method_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(input.fact_id)
    .bind(input.source_type_id)
    .bind(input.connector_instance_id)
    .bind(input.connector_type_id)
    .bind(raw_reference_norm)
    .bind(extracted_at)
    .bind(input.extraction_method_id)
    .fetch_one(pool)
    .await?;

    let source = sqlx::query_as::<_, Source>(
        "SELECT id, fact_id, source_type_id, connector_instance_id, connector_type_id, \
         raw_reference, extracted_at, extraction_method_id FROM sources WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    Ok(source)
}

/// Retrieve all sources linked to a fact.
pub async fn get_sources_for_fact(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Vec<Source>, KnowledgeError> {
    let sources: Vec<Source> = sqlx::query_as::<_, Source>(
        "SELECT id, fact_id, source_type_id, connector_instance_id, connector_type_id, \
         raw_reference, extracted_at, extraction_method_id FROM sources WHERE fact_id = ?",
    )
    .bind(fact_id)
    .fetch_all(pool)
    .await?;

    Ok(sources)
}

/// Add a new source to an existing fact and write a `source_added` audit entry.
pub async fn add_source_to_fact(
    pool: &SqlitePool,
    input: &SourceInput,
    extracted_at: DateTime<Utc>,
    changed_by: ChangedBy,
) -> Result<Source, KnowledgeError> {
    let mut tx = pool.begin().await?;

    let raw_reference_norm = input.raw_reference.as_deref().unwrap_or("");
    let changed_at = Utc::now();

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sources \
         (fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference, \
         extracted_at, extraction_method_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(input.fact_id)
    .bind(input.source_type_id)
    .bind(input.connector_instance_id)
    .bind(input.connector_type_id)
    .bind(raw_reference_norm)
    .bind(extracted_at)
    .bind(input.extraction_method_id)
    .fetch_one(&mut *tx)
    .await?;

    let source = sqlx::query_as::<_, Source>(
        "SELECT id, fact_id, source_type_id, connector_instance_id, connector_type_id, \
         raw_reference, extracted_at, extraction_method_id FROM sources WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;

    let new_value = serde_json::json!({
        "source_type_id": input.source_type_id,
        "connector_instance_id": input.connector_instance_id,
        "connector_type_id": input.connector_type_id,
        "raw_reference": raw_reference_norm,
        "extraction_method_id": input.extraction_method_id,
    })
    .to_string();

    sqlx::query(
        "INSERT INTO fact_audit_log \
         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(input.fact_id)
    .bind(ChangeType::SourceAdded as i16)
    .bind(None::<&str>)
    .bind(new_value)
    .bind(changed_at)
    .bind(changed_by as i16)
    .bind(None::<&str>)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(source)
}
