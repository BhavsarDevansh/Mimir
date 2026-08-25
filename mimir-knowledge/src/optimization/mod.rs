//! Nightly knowledge graph optimization runner.
//!
//! - `runbook` — optimization run lifecycle persistence.
//! - `passes` — the individual optimization pass implementations.
//! - `backup` — database backup creation and retention pruning.
//! - `nightly` — the scheduled nightly entry point.

use std::path::PathBuf;
use std::sync::Arc;

use mimir_core::llm::LlmBackend;

use crate::KnowledgeGraph;

mod backup;
mod nightly;
mod passes;
mod runbook;

pub use nightly::run_nightly_optimization;

/// Stable names for optimization passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassName {
    Deduplication,
    SemanticDeduplication,
    EntitySemanticDeduplication,
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
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Deduplication => "deduplication",
            Self::SemanticDeduplication => "semantic_deduplication",
            Self::EntitySemanticDeduplication => "entity_semantic_dedup",
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
    pub entity_merges_queued: u32,
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
                        PassName::EntitySemanticDeduplication,
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
}
