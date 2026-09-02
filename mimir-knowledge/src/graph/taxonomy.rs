//! Closed relationship taxonomy governance and unknown-fact staging (#468).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::KnowledgeError;
use crate::graph::KnowledgeGraph;

/// A durable record for an LLM-emitted fact that did not resolve to a
/// controlled relationship leaf.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct UnrecognizedFact {
    pub id: i64,
    pub connector_instance_id: Option<i32>,
    pub raw_reference: Option<String>,
    pub relationship_type_raw: String,
    pub payload_json: String,
    pub status: String,
    pub proposed_relationship_type_id: Option<i16>,
    pub resolution_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl KnowledgeGraph {
    /// List canonical leaf names eligible for LLM fact emission.
    ///
    /// The extraction tool schema uses this list directly, so a taxonomy
    /// migration or governance update becomes the single source of truth for
    /// both the emitted enum and the Rust-side resolver.
    pub async fn list_emit_eligible_relationship_type_names(
        &self,
    ) -> Result<Vec<String>, KnowledgeError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM relationship_types WHERE emit_eligible = TRUE ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }

    /// Store an unrecognized fact durably for governance instead of dropping
    /// it. The full producer payload is retained so approval can later map
    /// the fact without re-extraction.
    pub async fn stage_unrecognized_fact(
        &self,
        connector_instance_id: Option<i32>,
        raw_reference: Option<&str>,
        relationship_type_raw: &str,
        payload_json: &str,
        proposed_relationship_type_id: Option<i16>,
    ) -> Result<i64, KnowledgeError> {
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO unrecognized_facts (connector_instance_id, raw_reference, relationship_type_raw, payload_json, proposed_relationship_type_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (connector_instance_id, raw_reference, relationship_type_raw, payload_json) DO UPDATE SET updated_at = excluded.updated_at RETURNING id",
        )
        .bind(connector_instance_id)
        .bind(raw_reference)
        .bind(relationship_type_raw)
        .bind(payload_json)
        .bind(proposed_relationship_type_id)
        .bind(now)
        .bind(now)
            .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// List unrecognized facts, optionally filtered by governance status.
    pub async fn list_unrecognized_facts(
        &self,
        status: Option<&str>,
    ) -> Result<Vec<UnrecognizedFact>, KnowledgeError> {
        let rows = sqlx::query_as::<_, UnrecognizedFact>(
            "SELECT id, connector_instance_id, raw_reference, relationship_type_raw, payload_json, status, proposed_relationship_type_id, resolution_note, created_at, updated_at \
             FROM unrecognized_facts \
             WHERE (?1 IS NULL OR status = ?1) \
             ORDER BY created_at ASC, id ASC",
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Mark a staged fact as mapped to an existing controlled leaf.
    pub async fn resolve_unrecognized_fact(
        &self,
        id: i64,
        relationship_type_id: i16,
        note: Option<&str>,
    ) -> Result<(), KnowledgeError> {
        let target: Option<(bool,)> =
            sqlx::query_as("SELECT emit_eligible FROM relationship_types WHERE id = ?")
                .bind(relationship_type_id)
                .fetch_optional(&self.pool)
                .await?;
        if !target.is_some_and(|(emit_eligible,)| emit_eligible) {
            return Err(KnowledgeError::Validation(
                "staged facts can only be mapped to an emit-eligible taxonomy leaf".to_string(),
            ));
        }
        let result = sqlx::query(
            "UPDATE unrecognized_facts SET status = 'mapped', proposed_relationship_type_id = ?, resolution_note = ?, updated_at = ? WHERE id = ? AND status = 'unmapped'",
        )
        .bind(relationship_type_id)
        .bind(note)
        .bind(self.now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(KnowledgeError::Validation(format!(
                "unrecognized fact {id} is not unmapped"
            )));
        }
        Ok(())
    }

    /// Mark a staged fact as rejected by governance review.
    pub async fn reject_unrecognized_fact(
        &self,
        id: i64,
        note: Option<&str>,
    ) -> Result<(), KnowledgeError> {
        let result = sqlx::query(
            "UPDATE unrecognized_facts SET status = 'rejected', resolution_note = ?, updated_at = ? WHERE id = ? AND status = 'unmapped'",
        )
        .bind(note)
        .bind(self.now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(KnowledgeError::Validation(format!(
                "unrecognized fact {id} is not unmapped"
            )));
        }
        Ok(())
    }
}
