//! Optimization run lifecycle: run creation, pass recording, and resumption
//! of an interrupted scheduled run.

use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{OptimizationRunner, PassName, PassSummary};

impl<'a> OptimizationRunner<'a> {
    pub(super) async fn begin_run(&self, trigger: &str) -> Result<i64, crate::KnowledgeError> {
        let started_at = self.kg.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO optimization_runs (started_at, status, trigger) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(started_at)
        .bind("running")
        .bind(trigger)
        .fetch_one(self.kg.pool())
        .await?;
        Ok(id)
    }

    pub(super) async fn finish_run(
        &self,
        run_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), crate::KnowledgeError> {
        sqlx::query(
            "UPDATE optimization_runs SET status = ?, finished_at = ?, error = ? WHERE id = ?",
        )
        .bind(status)
        .bind(self.kg.now())
        .bind(error)
        .bind(run_id)
        .execute(self.kg.pool())
        .await?;
        Ok(())
    }

    /// Check for the most recent failed scheduled run and return its id plus
    /// the passes that still need to execute.
    pub(super) async fn resume_failed_run(
        &self,
    ) -> Result<Option<(i64, Vec<PassName>)>, crate::KnowledgeError> {
        let row = sqlx::query(
            "SELECT id FROM optimization_runs WHERE status = 'failed' AND trigger = 'scheduled' ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(self.kg.pool())
        .await?;
        let run_id: i64 = match row {
            Some(r) => r.try_get("id")?,
            None => return Ok(None),
        };
        let completed: Vec<String> = sqlx::query_scalar(
            "SELECT pass_name FROM optimization_pass_runs WHERE run_id = ? AND status = 'succeeded'",
        )
        .bind(run_id)
        .fetch_all(self.kg.pool())
        .await?;
        let completed_set: std::collections::HashSet<String> = completed.into_iter().collect();
        let all_passes = vec![
            PassName::Deduplication,
            PassName::SemanticDeduplication,
            PassName::EntitySemanticDeduplication,
            PassName::Contradiction,
            PassName::InferenceChain,
            PassName::ConfidenceRecalc,
            PassName::DormantCleanup,
            PassName::PatternConsolidation,
            PassName::PendingConfirmationCleanup,
            PassName::TrashCleanup,
            PassName::Compaction,
        ];
        let remaining: Vec<PassName> = all_passes
            .into_iter()
            .filter(|p| !completed_set.contains(p.as_str()))
            .collect();
        if remaining.is_empty() {
            // All passes were actually completed; mark the run as succeeded retroactively.
            self.finish_run(run_id, "succeeded", None).await?;
            return Ok(None);
        }
        Ok(Some((run_id, remaining)))
    }

    pub(super) async fn run_pass_with_run_id(
        &self,
        pass: PassName,
        run_id: i64,
    ) -> Result<PassSummary, crate::KnowledgeError> {
        let started_at = self.kg.now();
        let result = match pass {
            PassName::Deduplication => self.deterministic_dedup().await,
            PassName::SemanticDeduplication => self.semantic_dedup().await,
            PassName::EntitySemanticDeduplication => self.entity_semantic_dedup().await,
            PassName::Contradiction => self.contradiction().await,
            PassName::InferenceChain => self.inference_chain().await,
            PassName::ConfidenceRecalc => self.confidence_recalc().await,
            PassName::DormantCleanup => self.dormant_cleanup().await,
            PassName::PatternConsolidation => self.pattern_consolidation().await,
            PassName::PendingConfirmationCleanup => self.pending_confirmation_cleanup().await,
            PassName::TrashCleanup => self.trash_cleanup().await,
            PassName::Compaction => self.compaction().await,
        };

        match result {
            Ok(mut summary) => {
                summary.pass = Some(pass);
                self.record_pass(run_id, pass, started_at, &summary, None)
                    .await?;
                Ok(summary)
            }
            Err(e) => {
                self.record_pass(
                    run_id,
                    pass,
                    started_at,
                    &PassSummary::default(),
                    Some(&e.to_string()),
                )
                .await?;
                Err(e)
            }
        }
    }

    pub(super) async fn record_pass(
        &self,
        run_id: i64,
        pass: PassName,
        started_at: DateTime<Utc>,
        summary: &PassSummary,
        error: Option<&str>,
    ) -> Result<(), crate::KnowledgeError> {
        sqlx::query(
            "INSERT INTO optimization_pass_runs \
             (run_id, pass_name, status, started_at, finished_at, facts_merged, dedup_candidates_queued, entity_merges_queued, facts_forgotten, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(pass.as_str())
        .bind(if error.is_some() { "failed" } else { "succeeded" })
        .bind(started_at)
        .bind(self.kg.now())
        .bind(summary.facts_merged as i64)
        .bind(summary.dedup_candidates_queued as i64)
        .bind(summary.entity_merges_queued as i64)
        .bind(summary.facts_forgotten as i64)
        .bind(error)
        .execute(self.kg.pool())
        .await?;
        Ok(())
    }
}
