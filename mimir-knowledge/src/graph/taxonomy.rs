//! Closed relationship taxonomy governance and unknown-fact staging (#468).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::KnowledgeError;
use crate::graph::KnowledgeGraph;

/// An emit-eligible taxonomy leaf with the prompt-facing guidance text.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitEligiblePredicate {
    /// Canonical predicate name, as used by the `remember` tool schema enum.
    pub name: String,
    /// Name of the taxonomy root this leaf hangs from (empty when orphaned).
    pub root_name: String,
    /// Human-readable meaning, preferring the hand-written `description`
    /// and falling back to the closed-taxonomy `definition`.
    pub guidance: String,
}

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

/// The result of staging a producer payload. `newly_staged` lets callers
/// preserve idempotent review-queue counts when a work item is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnrecognizedFactStage {
    pub id: i64,
    pub newly_staged: bool,
}

impl KnowledgeGraph {
    /// List emit-eligible leaves with the text needed to render prompt
    /// guidance.
    ///
    /// The extraction tool schema and the extraction prompt both use this
    /// list, so a taxonomy migration or governance update becomes the single
    /// source of truth for the emitted enum and the prompt's predicate
    /// standards (issue #598).
    pub async fn list_emit_eligible_relationship_types(
        &self,
    ) -> Result<Vec<EmitEligiblePredicate>, KnowledgeError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT t.name, COALESCE(root.name, ''), \
                    COALESCE(NULLIF(t.description, ''), t.definition) \
             FROM relationship_types t \
             LEFT JOIN relationship_types root ON root.id = t.parent_id \
             WHERE t.emit_eligible = TRUE \
             ORDER BY COALESCE(root.name, ''), t.name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(name, root_name, guidance)| EmitEligiblePredicate {
                name,
                root_name,
                guidance,
            })
            .collect())
    }

    /// List canonical leaf names eligible for LLM fact emission.
    ///
    /// The extraction tool schema uses this list directly, so a taxonomy
    /// migration or governance update becomes the single source of truth for
    /// both the emitted enum and the Rust-side resolver.
    pub async fn list_emit_eligible_relationship_type_names(
        &self,
    ) -> Result<Vec<String>, KnowledgeError> {
        Ok(self
            .list_emit_eligible_relationship_types()
            .await?
            .into_iter()
            .map(|predicate| predicate.name)
            .collect())
    }

    /// Store an unrecognized fact durably for governance instead of dropping
    /// it. For connector rows, the staged counter is incremented in the same
    /// transaction, so a retryable later failure cannot lose the count. The
    /// full producer payload is retained so approval can later map the fact
    /// without re-extraction.
    pub async fn stage_unrecognized_fact(
        &self,
        connector_instance_id: Option<i32>,
        raw_reference: Option<&str>,
        relationship_type_raw: &str,
        payload_json: &str,
        proposed_relationship_type_id: Option<i16>,
    ) -> Result<UnrecognizedFactStage, KnowledgeError> {
        let now = self.now();
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO unrecognized_facts \
             (connector_instance_id, raw_reference, relationship_type_raw, payload_json, proposed_relationship_type_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(connector_instance_id)
        .bind(raw_reference)
        .bind(relationship_type_raw)
        .bind(payload_json)
        .bind(proposed_relationship_type_id)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let newly_staged = inserted.rows_affected() == 1;
        if newly_staged && connector_instance_id.is_some() {
            sqlx::query(
                "UPDATE connectors \
                 SET facts_staged = facts_staged + 1, updated_at = ? \
                 WHERE id = ?",
            )
            .bind(now)
            .bind(connector_instance_id)
            .execute(&mut *tx)
            .await?;
        }

        let id = if newly_staged {
            inserted.last_insert_rowid()
        } else {
            sqlx::query_scalar(
                "SELECT id FROM unrecognized_facts \
                 WHERE COALESCE(connector_instance_id, -1) = COALESCE(?, -1) \
                 AND COALESCE(raw_reference, '') = COALESCE(?, '') \
                 AND relationship_type_raw = ? AND payload_json = ?",
            )
            .bind(connector_instance_id)
            .bind(raw_reference)
            .bind(relationship_type_raw)
            .bind(payload_json)
            .fetch_one(&mut *tx)
            .await?
        };
        tx.commit().await?;
        Ok(UnrecognizedFactStage { id, newly_staged })
    }

    /// List unrecognized facts, optionally filtered by governance status.
    pub async fn list_unrecognized_facts(
        &self,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<UnrecognizedFact>, i64), KnowledgeError> {
        let mut tx = self.pool.begin().await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) \
             FROM unrecognized_facts \
             WHERE (?1 IS NULL OR status = ?1)",
        )
        .bind(status)
        .fetch_one(&mut *tx)
        .await?;
        let rows = sqlx::query_as::<_, UnrecognizedFact>(
            "SELECT id, connector_instance_id, raw_reference, relationship_type_raw, payload_json, status, proposed_relationship_type_id, resolution_note, created_at, updated_at \
             FROM unrecognized_facts \
             WHERE (?1 IS NULL OR status = ?1) \
             ORDER BY created_at ASC, id ASC \
             LIMIT ?2 OFFSET ?3",
        )
        .bind(status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((rows, total))
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
