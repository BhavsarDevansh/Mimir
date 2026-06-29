//! Nightly knowledge graph optimization runner.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use mimir_core::llm::LlmBackend;
use mimir_core::llm::types::Message;
use serde::Deserialize;
use sqlx::{Row, Sqlite, Transaction};

use crate::KnowledgeGraph;
use crate::inference::rules::contradiction::ContradictionRule;
use crate::inference::rules::threshold::ThresholdRule;
use crate::models::audit_log::{ChangeType, ChangedBy};
use crate::models::enums::RelationType;
use crate::models::fact::{FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};

/// Stable names for optimization passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassName {
    Deduplication,
    SemanticDeduplication,
    Contradiction,
    InferenceChain,
    ConfidenceRecalc,
    DormantCleanup,
    PatternConsolidation,
    PendingConfirmationCleanup,
    TrashCleanup,
    Compaction,
}

impl PassName {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deduplication => "deduplication",
            Self::SemanticDeduplication => "semantic_deduplication",
            Self::Contradiction => "contradiction",
            Self::InferenceChain => "inference_chain",
            Self::ConfidenceRecalc => "confidence_recalc",
            Self::DormantCleanup => "dormant_cleanup",
            Self::PatternConsolidation => "pattern_consolidation",
            Self::PendingConfirmationCleanup => "pending_confirmation_cleanup",
            Self::TrashCleanup => "trash_cleanup",
            Self::Compaction => "compaction",
        }
    }
}

/// Runtime options for an optimization run.
#[derive(Debug, Clone)]
pub struct OptimizationConfig {
    pub backup_dir: PathBuf,
    pub timeout_minutes: u16,
    pub schedule_time: String,
    /// Retention window (days) for the `pending_confirmation_cleanup` pass,
    /// mirrored from `knowledge.pending_cleanup.retention_days` so the
    /// optimization pass and the scheduled `knowledge.pending_cleanup` job
    /// share one configured expiry window.
    pub pending_cleanup_retention_days: u16,
}

impl OptimizationConfig {
    pub fn for_test(backup_dir: PathBuf) -> Self {
        Self {
            backup_dir,
            timeout_minutes: 120,
            schedule_time: "02:00".to_string(),
            pending_cleanup_retention_days: 7,
        }
    }
}

/// Per-pass mutation counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassSummary {
    pub pass: Option<PassName>,
    pub facts_merged: u32,
    pub dedup_candidates_queued: u32,
    pub facts_forgotten: u32,
}

/// Orchestrates the serial nightly optimization passes.
pub struct OptimizationRunner<'a> {
    kg: &'a KnowledgeGraph,
    config: OptimizationConfig,
    llm: Option<Arc<dyn LlmBackend>>,
}

impl<'a> OptimizationRunner<'a> {
    pub fn new(
        kg: &'a KnowledgeGraph,
        config: OptimizationConfig,
        llm: Option<Arc<dyn LlmBackend>>,
    ) -> Self {
        Self { kg, config, llm }
    }

    /// Execute a single pass and record its outcome.
    /// Execute a single pass and record its outcome.
    pub async fn run_pass(&self, pass: PassName) -> Result<PassSummary, crate::KnowledgeError> {
        let run_id = self.begin_run("manual").await?;
        let started_at = self.kg.now();
        let result = match pass {
            PassName::Deduplication => self.deterministic_dedup().await,
            PassName::SemanticDeduplication => self.semantic_dedup().await,
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
                self.finish_run(run_id, "succeeded", None).await?;
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
                self.finish_run(run_id, "failed", Some(&e.to_string()))
                    .await?;
                Err(e)
            }
        }
    }

    /// Execute the configured nightly pipeline.
    pub async fn run_all(&self) -> Result<Vec<PassSummary>, crate::KnowledgeError> {
        self.run_all_with_yield(|| false).await
    }

    /// Execute the pipeline with an async callback invoked on successful completion.
    pub async fn run_all_with_callback<F, C, CFut>(
        &self,
        mut should_yield: F,
        on_complete: C,
    ) -> Result<Vec<PassSummary>, crate::KnowledgeError>
    where
        F: FnMut() -> bool,
        C: FnOnce() -> CFut,
        CFut: std::future::Future<Output = ()> + Send,
    {
        let result = self.run_all_with_yield(&mut should_yield).await?;
        on_complete().await;
        Ok(result)
    }

    /// Execute the configured nightly pipeline, yielding between passes when
    /// `should_yield` returns `true`.
    /// Execute the configured nightly pipeline, yielding between passes when
    /// `should_yield` returns `true`.
    pub async fn run_all_with_yield<F>(
        &self,
        mut should_yield: F,
    ) -> Result<Vec<PassSummary>, crate::KnowledgeError>
    where
        F: FnMut() -> bool,
    {
        self.create_backup().await?;
        self.prune_backups().await?;
        let (run_id, passes) = match self.resume_failed_run().await? {
            Some((id, remaining)) => {
                tracing::info!("Resuming optimization run {} from failed pass", id);
                (id, remaining)
            }
            None => {
                let id = self.begin_run("scheduled").await?;
                (
                    id,
                    vec![
                        PassName::Deduplication,
                        PassName::SemanticDeduplication,
                        PassName::Contradiction,
                        PassName::InferenceChain,
                        PassName::ConfidenceRecalc,
                        PassName::DormantCleanup,
                        PassName::PatternConsolidation,
                        PassName::PendingConfirmationCleanup,
                        PassName::TrashCleanup,
                        PassName::Compaction,
                    ],
                )
            }
        };
        let mut summaries = Vec::new();
        let mut overall_error = None;
        for pass in passes {
            while should_yield() {
                // Best-effort yield: sleep briefly and recheck. The caller's
                // timeout (e.g. JobQueue) is the ultimate safety bound.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            match self.run_pass_with_run_id(pass, run_id).await {
                Ok(summary) => summaries.push(summary),
                Err(e) => {
                    overall_error = Some(e);
                    break;
                }
            }
        }
        let status = if overall_error.is_some() {
            "failed"
        } else {
            "succeeded"
        };
        self.finish_run(
            run_id,
            status,
            overall_error.as_ref().map(|e| e.to_string()).as_deref(),
        )
        .await?;
        if let Some(e) = overall_error {
            return Err(e);
        }
        Ok(summaries)
    }

    async fn begin_run(&self, trigger: &str) -> Result<i64, crate::KnowledgeError> {
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

    async fn finish_run(
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
    async fn resume_failed_run(
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

    async fn run_pass_with_run_id(
        &self,
        pass: PassName,
        run_id: i64,
    ) -> Result<PassSummary, crate::KnowledgeError> {
        let started_at = self.kg.now();
        let result = match pass {
            PassName::Deduplication => self.deterministic_dedup().await,
            PassName::SemanticDeduplication => self.semantic_dedup().await,
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

    async fn deterministic_dedup(&self) -> Result<PassSummary, crate::KnowledgeError> {
        let pairs = sqlx::query(
            "SELECT a.id AS keep_id, b.id AS duplicate_id \
             FROM facts a \
             JOIN facts b ON b.id > a.id \
              AND b.subject_id = a.subject_id \
              AND b.relationship_type_id = a.relationship_type_id \
              AND COALESCE(b.object_id, -1) = COALESCE(a.object_id, -1) \
              AND COALESCE(b.object_literal, '') = COALESCE(a.object_literal, '') \
              AND COALESCE(b.valid_from, '0001-01-01T00:00:00Z') <= COALESCE(a.valid_until, '9999-12-31T23:59:59Z') \
              AND COALESCE(a.valid_from, '0001-01-01T00:00:00Z') <= COALESCE(b.valid_until, '9999-12-31T23:59:59Z') \
             WHERE a.fact_status_id NOT IN (?, ?) AND b.fact_status_id NOT IN (?, ?)",
        )
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .fetch_all(self.kg.pool())
        .await?;

        let mut merged = 0;
        let mut seen = HashSet::new();
        for row in pairs {
            let keep_id: i32 = row.try_get("keep_id")?;
            let duplicate_id: i32 = row.try_get("duplicate_id")?;
            if !seen.insert(duplicate_id) {
                continue;
            }
            let mut tx = self.kg.pool().begin().await?;
            merge_fact_pair(&mut tx, self.kg.now(), keep_id, duplicate_id).await?;
            tx.commit().await?;
            merged += 1;
        }

        Ok(PassSummary {
            facts_merged: merged,
            ..PassSummary::default()
        })
    }

    async fn semantic_dedup(&self) -> Result<PassSummary, crate::KnowledgeError> {
        let Some(llm) = &self.llm else {
            tracing::warn!("semantic dedup skipped: no LLM backend configured");
            return Ok(PassSummary::default());
        };

        let candidates = sqlx::query(
            "SELECT a.id AS fact_a_id, b.id AS fact_b_id, \
                    rta.name AS predicate_a, rtb.name AS predicate_b, \
                    ea.name AS subject_name, COALESCE(ob.name, a.object_literal, '') AS object_name \
             FROM facts a \
             JOIN facts b ON b.id > a.id \
              AND b.subject_id = a.subject_id \
              AND COALESCE(b.object_id, -1) = COALESCE(a.object_id, -1) \
              AND COALESCE(b.object_literal, '') = COALESCE(a.object_literal, '') \
              AND b.relationship_type_id != a.relationship_type_id \
             JOIN relationship_types rta ON rta.id = a.relationship_type_id \
             JOIN relationship_types rtb ON rtb.id = b.relationship_type_id \
             JOIN entities ea ON ea.id = a.subject_id \
             LEFT JOIN entities ob ON ob.id = a.object_id \
             WHERE a.fact_status_id NOT IN (?, ?) AND b.fact_status_id NOT IN (?, ?) \
             ORDER BY a.id, b.id \
             LIMIT 50",
        )
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .fetch_all(self.kg.pool())
        .await?;

        if candidates.is_empty() {
            return Ok(PassSummary::default());
        }

        let candidate_json: Vec<serde_json::Value> = candidates
            .iter()
            .map(|row| {
                serde_json::json!({
                    "fact_a_id": row.try_get::<i32, _>("fact_a_id").unwrap_or_default(),
                    "fact_b_id": row.try_get::<i32, _>("fact_b_id").unwrap_or_default(),
                    "predicate_a": row.try_get::<String, _>("predicate_a").unwrap_or_default(),
                    "predicate_b": row.try_get::<String, _>("predicate_b").unwrap_or_default(),
                    "subject": row.try_get::<String, _>("subject_name").unwrap_or_default(),
                    "object": row.try_get::<String, _>("object_name").unwrap_or_default(),
                })
            })
            .collect();

        let tool = dedup_tool_schema();
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: "Use the evaluate_dedup_candidates tool to return your evaluation."
                    .to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "user".to_string(),
                content: serde_json::to_string(&candidate_json)
                    .map_err(|e| crate::KnowledgeError::Validation(e.to_string()))?,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let (assistant_msg, _) =
            llm.chat_message(messages, Some(vec![tool]))
                .await
                .map_err(|e| {
                    crate::KnowledgeError::Validation(format!("semantic dedup LLM error: {e}"))
                })?;

        let tool_calls = assistant_msg.tool_calls.as_ref().ok_or_else(|| {
            crate::KnowledgeError::Validation(
                "semantic dedup: no tool calls in LLM response".to_string(),
            )
        })?;
        let first = tool_calls.first().ok_or_else(|| {
            crate::KnowledgeError::Validation(
                "semantic dedup: empty tool calls in LLM response".to_string(),
            )
        })?;
        let response: SemanticDedupResponse = serde_json::from_str(&first.function.arguments)
            .map_err(|e| {
                crate::KnowledgeError::Validation(format!("semantic dedup JSON error: {e}"))
            })?;

        let valid_pairs: HashSet<(i32, i32)> = candidates
            .iter()
            .filter_map(|row| {
                Some((
                    row.try_get("fact_a_id").ok()?,
                    row.try_get("fact_b_id").ok()?,
                ))
            })
            .collect();

        let mut queued = 0;
        for candidate in response.candidates {
            let pair = ordered_pair(candidate.fact_a_id, candidate.fact_b_id);
            if !valid_pairs.contains(&pair) {
                continue;
            }
            if candidate.suggested_action == "merge" && candidate.llm_confidence >= 0.9 {
                let mut tx = self.kg.pool().begin().await?;
                merge_fact_pair(&mut tx, self.kg.now(), pair.0, pair.1).await?;
                tx.commit().await?;
            } else {
                sqlx::query(
                    "INSERT INTO dedup_queue \
                     (fact_id, fact_b_id, status_id, queued_at, suggested_action, llm_confidence) \
                     VALUES (?, ?, 1, ?, ?, ?)",
                )
                .bind(pair.0)
                .bind(pair.1)
                .bind(self.kg.now())
                .bind(candidate.suggested_action)
                .bind(candidate.llm_confidence)
                .execute(self.kg.pool())
                .await?;
                queued += 1;
            }
        }

        Ok(PassSummary {
            dedup_candidates_queued: queued,
            ..PassSummary::default()
        })
    }

    async fn contradiction(&self) -> Result<PassSummary, crate::KnowledgeError> {
        ContradictionRule::evaluate_batch(self.kg).await?;
        Ok(PassSummary::default())
    }

    async fn inference_chain(&self) -> Result<PassSummary, crate::KnowledgeError> {
        let mut engine = crate::inference::RuleEngine::new();
        engine.register(Box::new(
            crate::inference::rules::transitivity::TransitivityRule,
        ));
        engine.register(Box::new(
            crate::inference::rules::contradiction::ContradictionRule,
        ));

        let inferred = engine.evaluate_batch(self.kg).await?;
        for mut new_fact in inferred {
            new_fact.inferred = true;
            new_fact.source_type = SourceType::Inference;
            new_fact.extraction_method = Some(ExtractionMethod::InferenceRule);
            let relationship_type_id = self
                .kg
                .ensure_relationship_type(&new_fact.relationship_type)
                .await?;
            if fact_already_exists(self.kg, &new_fact, relationship_type_id).await? {
                continue;
            }
            let mut ctx = crate::inference::CascadeContext::new();
            self.kg.insert_fact_internal(new_fact, &mut ctx).await?;
        }

        ThresholdRule::evaluate_batch(self.kg).await?;
        Ok(PassSummary::default())
    }

    async fn confidence_recalc(&self) -> Result<PassSummary, crate::KnowledgeError> {
        let stale_facts: Vec<i32> =
            sqlx::query_scalar("SELECT id FROM facts WHERE stale_confidence = TRUE")
                .fetch_all(self.kg.pool())
                .await?;

        // Root-aware recalculation: each stale fact is recalculated/cleared
        // itself (not just its descendants) inside one transaction, so the
        // nightly pass can no longer leave the selected rows stale forever.
        // Because each pass also cascades to inferred descendants and clears
        // their stale flags, an earlier iteration may have already refreshed
        // a fact that still appears in this snapshot. Re-check staleness
        // cheaply before reopening a transaction so already-cleared subtrees
        // are not revisited, avoiding quadratic work on large stale branches.
        for fact_id in stale_facts {
            let still_stale: Option<bool> =
                sqlx::query_scalar("SELECT stale_confidence FROM facts WHERE id = ?")
                    .bind(fact_id)
                    .fetch_optional(self.kg.pool())
                    .await?;
            if !still_stale.unwrap_or(false) {
                continue;
            }
            crate::confidence::recalculate_stale_fact(self.kg.pool(), fact_id).await?;
        }
        Ok(PassSummary::default())
    }

    async fn dormant_cleanup(&self) -> Result<PassSummary, crate::KnowledgeError> {
        let cutoff = self.kg.now() - chrono::Duration::days(30);
        let fact_ids: Vec<i32> = sqlx::query_scalar(
            "SELECT DISTINCT f.id \
             FROM facts f \
             WHERE f.fact_status_id = ? \
               AND f.updated_at < ? \
               AND NOT EXISTS (
                   SELECT 1 FROM sources s
                   WHERE s.fact_id = f.id AND s.source_type_id = ?
               ) \
               AND EXISTS (
                   SELECT 1 FROM facts c
                   WHERE c.id != f.id
                     AND c.subject_id = f.subject_id
                     AND c.relationship_type_id = f.relationship_type_id
                     AND c.fact_status_id = ?
                     AND c.confidence > f.confidence
               )",
        )
        .bind(FactStatus::Disputed as i16)
        .bind(cutoff)
        .bind(SourceType::UserEdit as i16)
        .bind(FactStatus::Disputed as i16)
        .fetch_all(self.kg.pool())
        .await?;

        let mut forgotten = 0;
        for fact_id in fact_ids {
            crate::forget::forget_fact(
                self.kg.pool(),
                fact_id,
                ChangedBy::NightlyOptimization,
                self.kg.now(),
            )
            .await?;
            forgotten += 1;
        }

        Ok(PassSummary {
            facts_forgotten: forgotten,
            ..PassSummary::default()
        })
    }

    async fn pattern_consolidation(&self) -> Result<PassSummary, crate::KnowledgeError> {
        tracing::info!("pattern consolidation not yet implemented");
        Ok(PassSummary::default())
    }

    async fn compaction(&self) -> Result<PassSummary, crate::KnowledgeError> {
        sqlx::query("INSERT INTO entity_fts(entity_fts) VALUES('rebuild')")
            .execute(self.kg.pool())
            .await?;
        sqlx::query("ANALYZE").execute(self.kg.pool()).await?;
        sqlx::query("VACUUM").execute(self.kg.pool()).await?;
        Ok(PassSummary::default())
    }

    /// Remove old backups, keeping the 7 most recent.
    async fn prune_backups(&self) -> Result<(), crate::KnowledgeError> {
        let mut entries = tokio::fs::read_dir(&self.config.backup_dir).await?;
        let mut backups = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("knowledge-") && name.ends_with(".db") {
                let meta = entry.metadata().await?;
                if let Ok(modified) = meta.modified() {
                    backups.push((name, modified));
                }
            }
        }
        backups.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (name, _) in backups.into_iter().skip(7) {
            let path = self.config.backup_dir.join(&name);
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!("Failed to remove old backup {}: {}", path.display(), e);
            }
        }
        Ok(())
    }

    async fn create_backup(&self) -> Result<(), crate::KnowledgeError> {
        tokio::fs::create_dir_all(&self.config.backup_dir).await?;
        let date = self.kg.now().date_naive();
        let mut backup = self
            .config
            .backup_dir
            .join(format!("knowledge-{}.db", date));
        let mut counter = 1u32;
        while tokio::fs::try_exists(&backup).await.unwrap_or(false) {
            backup = self
                .config
                .backup_dir
                .join(format!("knowledge-{}-{}.db", date, counter));
            counter += 1;
        }
        let escaped = backup.to_string_lossy().replace('\'', "''");
        sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{}'", escaped)))
            .execute(self.kg.pool())
            .await?;
        Ok(())
    }

    async fn pending_confirmation_cleanup(&self) -> Result<PassSummary, crate::KnowledgeError> {
        // Delegates to the shared auto-expiry implementation (single source of
        // truth; also used by the `knowledge.pending_cleanup` daily job).
        let deleted = self
            .kg
            .delete_stale_pending(self.config.pending_cleanup_retention_days)
            .await?;
        Ok(PassSummary {
            facts_forgotten: deleted,
            ..PassSummary::default()
        })
    }

    async fn trash_cleanup(&self) -> Result<PassSummary, crate::KnowledgeError> {
        let deleted =
            crate::queries::trash::hard_delete_expired_trash(self.kg.pool(), self.kg.now()).await?;
        Ok(PassSummary {
            facts_forgotten: deleted as u32,
            ..PassSummary::default()
        })
    }

    async fn record_pass(
        &self,
        run_id: i64,
        pass: PassName,
        started_at: DateTime<Utc>,
        summary: &PassSummary,
        error: Option<&str>,
    ) -> Result<(), crate::KnowledgeError> {
        sqlx::query(
            "INSERT INTO optimization_pass_runs \
             (run_id, pass_name, status, started_at, finished_at, facts_merged, dedup_candidates_queued, facts_forgotten, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(pass.as_str())
        .bind(if error.is_some() { "failed" } else { "succeeded" })
        .bind(started_at)
        .bind(self.kg.now())
        .bind(summary.facts_merged as i64)
        .bind(summary.dedup_candidates_queued as i64)
        .bind(summary.facts_forgotten as i64)
        .bind(error)
        .execute(self.kg.pool())
        .await?;
        Ok(())
    }
}

/// JSON Schema for the `evaluate_dedup_candidates` tool.
fn dedup_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "evaluate_dedup_candidates",
            "description": "Evaluate candidate fact pairs for semantic deduplication and return structured results.",
            "parameters": {
                "type": "object",
                "properties": {
                    "candidates": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "fact_a_id": { "type": "integer" },
                                "fact_b_id": { "type": "integer" },
                                "suggested_action": {
                                    "type": "string",
                                    "enum": ["merge", "keep_separate"]
                                },
                                "llm_confidence": { "type": "number" }
                            },
                            "required": ["fact_a_id", "fact_b_id", "suggested_action", "llm_confidence"]
                        }
                    }
                },
                "required": ["candidates"]
            }
        }
    })
}

#[derive(Debug, Deserialize)]
struct SemanticDedupResponse {
    candidates: Vec<SemanticDedupCandidate>,
}

#[derive(Debug, Deserialize)]
struct SemanticDedupCandidate {
    fact_a_id: i32,
    fact_b_id: i32,
    suggested_action: String,
    llm_confidence: f32,
}

fn ordered_pair(a: i32, b: i32) -> (i32, i32) {
    if a <= b { (a, b) } else { (b, a) }
}

async fn merge_fact_pair(
    tx: &mut Transaction<'_, Sqlite>,
    now: DateTime<Utc>,
    keep_id: i32,
    duplicate_id: i32,
) -> Result<(), crate::KnowledgeError> {
    let keep_confidence: f32 = sqlx::query_scalar("SELECT confidence FROM facts WHERE id = ?")
        .bind(keep_id)
        .fetch_one(&mut **tx)
        .await?;
    let duplicate_confidence: f32 = sqlx::query_scalar("SELECT confidence FROM facts WHERE id = ?")
        .bind(duplicate_id)
        .fetch_one(&mut **tx)
        .await?;
    let boosted = (keep_confidence.max(duplicate_confidence) + 0.05).min(0.95);

    sqlx::query("UPDATE facts SET confidence = ?, updated_at = ? WHERE id = ?")
        .bind(boosted)
        .bind(now)
        .bind(keep_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO sources \
         (fact_id, source_type_id, connector_id, connector_type_id, raw_reference, extracted_at, extraction_method_id) \
         SELECT ?, source_type_id, connector_id, connector_type_id, raw_reference, extracted_at, extraction_method_id \
         FROM sources WHERE fact_id = ?",
    )
    .bind(keep_id)
    .bind(duplicate_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query("DELETE FROM sources WHERE fact_id = ?")
        .bind(duplicate_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO fact_dependencies \
         (parent_fact_id, child_fact_id, relation_type_id, is_positive) VALUES (?, ?, ?, TRUE)",
    )
    .bind(duplicate_id)
    .bind(keep_id)
    .bind(RelationType::Supersedes as i16)
    .execute(&mut **tx)
    .await?;

    sqlx::query("UPDATE facts SET fact_status_id = ?, updated_at = ? WHERE id = ?")
        .bind(FactStatus::Superseded as i16)
        .bind(now)
        .bind(duplicate_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT INTO fact_audit_log \
         (fact_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(duplicate_id)
    .bind(ChangeType::StatusChange as i16)
    .bind(None::<&str>)
    .bind(serde_json::json!({"fact_status_id": FactStatus::Superseded as i16}).to_string())
    .bind(now)
    .bind(ChangedBy::NightlyOptimization as i16)
    .bind(Some("Merged during nightly deduplication"))
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Check whether any fact with the same triple already exists.
async fn fact_already_exists(
    kg: &KnowledgeGraph,
    new_fact: &NewFact,
    relationship_type_id: i16,
) -> Result<bool, crate::KnowledgeError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ? \
           AND (object_id IS ?) AND (object_literal IS ?)",
    )
    .bind(new_fact.subject_id)
    .bind(relationship_type_id)
    .bind(new_fact.object_id)
    .bind(&new_fact.object_literal)
    .fetch_one(kg.pool())
    .await?;
    Ok(count > 0)
}

/// Compatibility wrapper for older callers.
pub async fn run_nightly_optimization(kg: &KnowledgeGraph) -> Result<(), crate::KnowledgeError> {
    let backup_dir = mimir_core::paths::data_dir()
        .map(|p| p.join("backups"))
        .map_err(|e| crate::KnowledgeError::Validation(e.to_string()))?;
    let runner = OptimizationRunner::new(kg, OptimizationConfig::for_test(backup_dir), None);
    runner.run_all().await?;
    Ok(())
}
