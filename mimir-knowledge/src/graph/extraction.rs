use crate::graph::KnowledgeGraph;
use crate::*;

use std::sync::Arc;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Fact extraction pipeline delegates
    // ------------------------------------------------------------------

    /// Extract facts from a user message via LLM, validate, and insert.
    pub async fn extract_facts(
        &self,
        llm: &Arc<dyn mimir_core::llm::backend::LlmBackend>,
        user_message: &str,
    ) -> Result<extract::ExtractionOutcome, KnowledgeError> {
        extract::extract_facts(self, llm, user_message).await
    }

    /// Extract facts from a labelled conversation transcript with the
    /// condensed core-facts block injected into the prompt.
    pub async fn extract_facts_with_context(
        &self,
        llm: &Arc<dyn mimir_core::llm::backend::LlmBackend>,
        messages: &[mimir_core::conversation::ConversationMessage],
        condensed_memory: Option<&str>,
    ) -> Result<extract::ExtractionOutcome, KnowledgeError> {
        extract::extract_facts_with_context(self, llm, messages, condensed_memory).await
    }

    /// Confirm a pending sensitive fact: flip to Active with confidence 1.0.
    pub async fn confirm_fact(&self, fact_id: i32) -> Result<models::fact::Fact, KnowledgeError> {
        extract::confirm_fact(self, fact_id).await
    }

    /// Reject a pending sensitive fact: hard-delete with audit trail.
    ///
    /// `reason`, if `Some`, overrides the default audit message. Convenience
    /// wrapper for the common no-reason case; see [`extract::reject_fact`].
    pub async fn reject_fact(
        &self,
        fact_id: i32,
        reason: Option<&str>,
    ) -> Result<(), KnowledgeError> {
        extract::reject_fact(self, fact_id, reason).await
    }

    /// List all facts awaiting user confirmation, with resolved subject,
    /// predicate, and object names. Backs `GET /kb/pending`.
    pub async fn list_pending_facts(
        &self,
    ) -> Result<Vec<queries::fact::PendingFactRow>, KnowledgeError> {
        queries::fact::list_pending(&self.pool).await
    }

    /// Hard-delete facts still awaiting confirmation older than `retention_days`
    /// relative to the configured clock, returning the number deleted.
    ///
    /// For each stale fact: removes `fact_dependencies` rows (RESTRICT FK),
    /// writes a `Rejected` audit entry attributed to `NightlyOptimization`,
    /// hard-deletes the fact, and syncs the in-memory `pending_confirmations`
    /// cache. The stale predicate is re-checked inside each per-fact
    /// transaction so a fact confirmed/rejected between the id scan and the
    /// delete is skipped (no spurious audit entry, no overwriting of a
    /// concurrent state change); only committed deletes are counted. Uses
    /// `self.now()` so tests can fast-forward via a [`clock::MockClock`].
    ///
    /// Backs the `knowledge.pending_cleanup` background job and the
    /// optimization runner's `pending_confirmation_cleanup` pass (single source
    /// of truth for the auto-expiry rule described in
    /// `VISION/02-Knowledge-Graph/Learning-Modes.md`).
    pub async fn delete_stale_pending(&self, retention_days: u16) -> Result<u32, KnowledgeError> {
        use crate::models::audit_log::{ChangeType, ChangedBy};

        let now = self.now();
        let cutoff = now - chrono::Duration::days(i64::from(retention_days));
        let reason = format!("Auto-expired after {retention_days} days without confirmation");

        let stale_ids: Vec<i32> = sqlx::query_scalar(
            "SELECT id FROM facts WHERE pending_confirmation = TRUE AND created_at < ?",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        let mut deleted = 0_u32;
        for fact_id in &stale_ids {
            let mut tx = self.pool().begin().await?;
            // Re-check the stale predicate inside the transaction. A fact
            // confirmed or rejected between the id scan above and this delete
            // must be skipped rather than incorrectly hard-deleted and audited.
            let still_stale: Option<i32> = sqlx::query_scalar(
                "SELECT id FROM facts \
                 WHERE id = ? AND pending_confirmation = TRUE AND created_at < ?",
            )
            .bind(fact_id)
            .bind(cutoff)
            .fetch_optional(&mut *tx)
            .await?;
            if still_stale.is_none() {
                tx.rollback().await?;
                continue;
            }

            sqlx::query(
                "DELETE FROM fact_dependencies WHERE parent_fact_id = ? OR child_fact_id = ?",
            )
            .bind(fact_id)
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO fact_audit_log                  (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason)                  VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(fact_id)
            .bind(ChangeType::Rejected as i16)
            .bind(None::<&str>)
            .bind(None::<&str>)
            .bind(now)
            .bind(ChangedBy::NightlyOptimization as i16)
            .bind(&reason)
            .execute(&mut *tx)
            .await?;
            // Guard the delete with the stale predicate so a concurrent
            // confirm/reject is never overwritten; only committed deletes
            // are counted.
            let result = sqlx::query(
                "DELETE FROM facts WHERE id = ? AND pending_confirmation = TRUE AND created_at < ?",
            )
            .bind(fact_id)
            .bind(cutoff)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                tx.rollback().await?;
                continue;
            }
            tx.commit().await?;
            self.pending_confirmations().write().await.remove(fact_id);
            deleted += 1;
        }

        Ok(deleted)
    }
}
