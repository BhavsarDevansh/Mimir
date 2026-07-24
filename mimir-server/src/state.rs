use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use chrono::Utc;
use dashmap::DashMap;

use mimir_core::{
    agents::AgentRuntime,
    config::ReloadableConfig,
    context::ContextManager,
    job_queue::{Job, JobContext, JobPriority, JobQueue},
    llm::{LlmBackend, LlmClient},
    scheduler::{BackgroundScheduler, DaemonJob},
    tools::ToolRegistry,
};

/// Shared application state for the HTTP server.
///
/// Holds all long-lived resources: the LLM client (backed by a worker pool),
/// the conversation context manager, per-session semaphores
/// to prevent concurrent mutation of the same session.
#[derive(Debug, Clone)]
pub struct AppState {
    pub llm_client: Arc<dyn LlmBackend>,
    pub context_manager: Arc<ContextManager>,

    /// Live reloadable configuration.
    pub config: Arc<ReloadableConfig>,
    /// Per-session semaphore to serialise concurrent requests for the same session.
    pub session_locks: Arc<DashMap<i64, Arc<tokio::sync::Semaphore>>>,
    pub start_time: Instant,
    /// LLM endpoint URL (for status reporting).
    pub endpoint: String,
    /// Configured LLM model (for status reporting).
    pub model: String,
    /// Shutdown signal sender.
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Cache for model-override LLM clients to avoid allocating a new client
    /// on every request with the same override model.
    pub model_override_cache: Arc<DashMap<String, Arc<dyn LlmBackend>>>,
    /// Tool registry for function-calling support.
    pub tool_registry: Arc<ToolRegistry>,
    /// Knowledge graph for entity and fact queries.
    pub knowledge_graph: Arc<mimir_knowledge::KnowledgeGraph>,
    /// Durable job queue for background tasks.
    pub job_queue: Arc<JobQueue>,
    /// In-memory agent runtime for background autonomous agents.
    pub agent_runtime: Arc<AgentRuntime>,
    /// Unified background scheduler (dedupe, debounce, idle-gate).
    pub scheduler: Arc<BackgroundScheduler>,
    /// Unix timestamp (seconds) of the last user interaction. Used to yield
    /// system jobs when the user is active.
    pub last_user_activity: Arc<AtomicU64>,
    /// Cached user entity ID in the knowledge graph (resolved at startup).
    pub user_entity_id: Option<i32>,
}

const MODEL_OVERRIDE_CACHE_CAP: usize = 16;
/// Maximum number of facts an entity may have to be considered an accidental
/// duplicate during auto-merge in `seed_identity_facts`. Entities with more
/// facts than this threshold are assumed to be intentional, distinct records.
const ACCIDENTAL_DUPLICATE_FACT_THRESHOLD: i64 = 2;

/// Log a warning for a failed operation and return `None`, or pass through the
/// success value.
///
/// Used for optional best-effort operations (tool registration, alias wiring,
/// auto-merge) where failure should not abort the caller.
fn warn_err<T, E: std::fmt::Display>(result: Result<T, E>, message: &str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!("{message}: {error}");
            None
        }
    }
}

/// Register a native tool, logging a warning on failure.
fn register_tool(registry: &ToolRegistry, tool: Arc<dyn mimir_core::tools::Tool>) {
    let name = tool.name().to_string();
    warn_err(
        registry.register_native(tool),
        &format!("Failed to register {name} tool"),
    );
}

impl AppState {
    /// Build `AppState` from the global [`ReloadableConfig`].
    pub async fn from_config(
        config: Arc<ReloadableConfig>,
    ) -> anyhow::Result<(Self, tokio::sync::watch::Receiver<bool>)> {
        let llm_client: Arc<dyn LlmBackend> =
            Arc::new(LlmClient::new(config.snapshot().await.llm.clone()).await?);
        Self::from_config_with_llm(config, llm_client).await
    }

    /// Build `AppState` from [`ReloadableConfig`] with an injected LLM backend.
    ///
    /// Primarily useful for tests that need to supply a [`MockLlmClient`]
    /// without relying on sentinel strings or config hacks.
    pub async fn from_config_with_llm(
        config: Arc<ReloadableConfig>,
        llm_client: Arc<dyn LlmBackend>,
    ) -> anyhow::Result<(Self, tokio::sync::watch::Receiver<bool>)> {
        let cfg = config.snapshot().await;

        let db_path = match cfg.context.db_path.clone() {
            Some(p) => p,
            None => mimir_core::paths::default_db_path()?,
        };
        let context_manager = Arc::new(ContextManager::new(&db_path).await?);

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let tool_registry = Arc::new(ToolRegistry::new());
        register_tool(
            &tool_registry,
            Arc::new(mimir_core::tools::GetCurrentTimeTool),
        );
        register_tool(&tool_registry, Arc::new(mimir_core::tools::EchoTool));
        register_tool(
            &tool_registry,
            Arc::new(mimir_core::tools::GetWeatherTool::new()),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_core::tools::SearchConversationHistoryTool::new(
                Arc::clone(&context_manager),
            )),
        );
        if let Some(path) = mimir_core::tools::ToolsConfig::default_path()
            && path.exists()
        {
            warn_err(
                tool_registry.load_tools_config(&path),
                "Failed to load tools config",
            );
        }

        // Initialise knowledge graph.
        let kg_db_path = mimir_core::paths::knowledge_db_path()?;
        let mut knowledge_graph = mimir_knowledge::KnowledgeGraph::init(&kg_db_path).await?;

        // Inject the default OSM Nominatim geocoder so the entity-locations
        // write path (Phase 3 S3 / #193) can fill the missing half of a
        // location (address -> coords or coords -> address). The backend does
        // no network work until a location fact is actually processed, so this
        // is cheap at startup. A config toggle / self-hosted endpoint can
        // follow; for now the policy-compliant public-instance defaults apply.
        // Construction only fails if the HTTP client or rate limiter cannot be
        // built, in which case geocoding is disabled (locations still persist
        // with whatever data the producer supplied) rather than aborting start.
        match mimir_connectors::NominatimGeocoder::with_defaults() {
            Ok(geocoder) => {
                knowledge_graph.set_geocoder(std::sync::Arc::new(geocoder));
                tracing::info!("Nominatim geocoder enabled for entity-locations write path");
            }
            Err(error) => tracing::warn!(
                "failed to initialise Nominatim geocoder; location geocoding disabled: {error}"
            ),
        }
        let knowledge_graph = Arc::new(knowledge_graph);

        // Resolve user entity from config identity.
        let user_entity_id = if cfg.identity.name.trim().is_empty() {
            tracing::warn!(
                "identity.name is empty; skipping user entity resolution. memory condensation will not run."
            );
            None
        } else {
            match knowledge_graph.search_entities(&cfg.identity.name, 1).await {
                Ok(mut results) if !results.is_empty() => {
                    tracing::info!(
                        "Resolved user entity: {} (id={})",
                        results[0].entity.name,
                        results[0].entity.id
                    );
                    Some(results.remove(0).entity.id)
                }
                Ok(_) => {
                    tracing::info!(
                        "User entity '{}' not found; creating as User type",
                        cfg.identity.name
                    );
                    match knowledge_graph
                        .create_entity(
                            &cfg.identity.name,
                            mimir_knowledge::models::entity::EntityType::Person,
                            &[],
                        )
                        .await
                    {
                        Ok(entity) => Some(entity.id),
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create user entity: {}; condensation disabled",
                                e
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to resolve user entity '{}': {}; condensation disabled",
                        cfg.identity.name,
                        e
                    );
                    None
                }
            }
        };

        // Seed identity facts for the user entity so Mimir knows the user's name.
        if let Some(uid) = user_entity_id {
            let name = cfg.identity.name.trim();
            let preferred = cfg.identity.preferred_name.trim();
            if let Err(e) = seed_identity_facts(&knowledge_graph, uid, name, preferred).await {
                tracing::warn!("Failed to seed identity facts: {}", e);
            }
        }

        // Register knowledge graph tools.
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::KgQueryTool::new(Arc::clone(
                &knowledge_graph,
            ))),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::KgRelatedTool::new(Arc::clone(
                &knowledge_graph,
            ))),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::KgSearchTool::new(Arc::clone(
                &knowledge_graph,
            ))),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::KgExpandCatalogueTool::new(Arc::clone(
                &knowledge_graph,
            ))),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::KgFactsInCatalogueTool::new(Arc::clone(
                &knowledge_graph,
            ))),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::RememberTool::new(Arc::clone(
                &knowledge_graph,
            ))),
        );
        register_tool(
            &tool_registry,
            Arc::new(mimir_knowledge::RetrieveContextTool::new(
                Arc::clone(&knowledge_graph),
                Arc::clone(&context_manager),
                Arc::clone(&llm_client),
            )),
        );

        // Initialise job queue.
        let jobs_db_path = mimir_core::paths::jobs_db_path()?;
        let job_queue = Arc::new(JobQueue::init(&jobs_db_path).await?);

        // Initialise agent runtime. The LibrarianAgent is registered so it
        // remains available for future on-demand/bulk extraction, but it is no
        // longer auto-invoked from the chat route (issue #137; learning is now
        // LLM-orchestrated via the `remember` tool).
        let agent_runtime = Arc::new(AgentRuntime::new());
        agent_runtime
            .register::<mimir_knowledge::librarian::LibrarianAgent>(
                mimir_knowledge::librarian::LibrarianAgent::new(),
            )
            .await;
        let last_user_activity = Arc::new(AtomicU64::new(0));

        // Initialise background scheduler.
        let scheduler_cfg = cfg.scheduler.clone();
        let (scheduler, scheduler_shutdown_rx) = BackgroundScheduler::new(
            Arc::clone(&job_queue),
            Arc::clone(&llm_client),
            std::time::Duration::from_secs(scheduler_cfg.debounce_seconds as u64),
            std::time::Duration::from_secs(scheduler_cfg.cooldown_seconds as u64),
        );

        // Register knowledge graph optimization job.
        let kg_for_job = Arc::clone(&knowledge_graph);
        let llm_for_job = Arc::clone(&llm_client);
        let activity_for_job = Arc::clone(&last_user_activity);
        let backup_dir = mimir_core::paths::data_dir()?.join("backups");
        let timeout_minutes = cfg.knowledge.optimization.timeout_minutes;
        let schedule_time = cfg.knowledge.optimization.schedule_time.clone();
        let pending_cleanup_retention_days = cfg.knowledge.pending_cleanup.retention_days;
        let schedule =
            mimir_core::job_queue::DailySchedule::parse(&cfg.knowledge.optimization.schedule_time)?;

        let scheduler_for_opt = Arc::clone(&scheduler);
        let opt_job = Job::new(
            "knowledge.optimization",
            JobPriority::System,
            Some(schedule),
            true,
            move |_ctx: JobContext| {
                let kg = Arc::clone(&kg_for_job);
                let llm = Arc::clone(&llm_for_job);
                let activity = Arc::clone(&activity_for_job);
                let scheduler = Arc::clone(&scheduler_for_opt);
                let backup_dir = backup_dir.clone();
                let timeout = timeout_minutes;
                let schedule_time = schedule_time.clone();
                let pending_cleanup_retention = pending_cleanup_retention_days;
                Box::pin(async move {
                    let opt_config = mimir_knowledge::optimization::OptimizationConfig {
                        backup_dir,
                        timeout_minutes: timeout,
                        schedule_time,
                        pending_cleanup_retention_days: pending_cleanup_retention,
                    };
                    let runner = mimir_knowledge::optimization::OptimizationRunner::new(
                        &kg,
                        opt_config,
                        Some(llm),
                    );
                    let five_minutes = chrono::Duration::minutes(5);
                    runner
                        .run_all_with_callback(
                            || {
                                let last = chrono::DateTime::from_timestamp(
                                    activity.load(Ordering::Relaxed) as i64,
                                    0,
                                )
                                .unwrap_or_else(|| Utc::now() - chrono::Duration::days(1));
                                Utc::now() - last < five_minutes
                            },
                            || async move {
                                scheduler.submit(DaemonJob::MemoryCondensation).await;
                            },
                        )
                        .await
                        .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                    Ok(())
                })
            },
        );
        job_queue.register(opt_job).await?;

        // Register memory condensation job.
        let kg_for_cond = Arc::clone(&knowledge_graph);
        let llm_for_cond = Arc::clone(&llm_client);
        let user_id_for_cond = user_entity_id;
        let char_limit = cfg.memory.char_limit as usize;
        let top_n = cfg.memory.condensation_top_n as usize;

        let cond_job = Job::new(
            "memory.condensation",
            JobPriority::System,
            None,
            true,
            move |_ctx: JobContext| {
                let kg = Arc::clone(&kg_for_cond);
                let llm = Arc::clone(&llm_for_cond);
                let uid = user_id_for_cond;
                let limit = char_limit;
                let top_n = top_n;
                Box::pin(async move {
                    if let Some(subject_id) = uid {
                        let condenser = mimir_knowledge::condensation::MemoryCondenser::new(
                            kg, llm, subject_id, limit, top_n,
                        );
                        condenser
                            .run()
                            .await
                            .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                    } else {
                        tracing::debug!("memory.condensation: no user entity configured; skipping");
                    }
                    Ok(())
                })
            },
        );
        job_queue.register(cond_job).await?;

        // Register the pending sensitive-fact auto-cleanup job (issue #141).
        // Hard-deletes facts still awaiting confirmation past the configured
        // retention window, writing a `Rejected` audit entry per fact. Daily,
        // idle-gated; shares its deletion logic with the optimization runner's
        // `pending_confirmation_cleanup` pass via `delete_stale_pending`.
        let kg_for_cleanup = Arc::clone(&knowledge_graph);
        let cleanup_retention_days = cfg.knowledge.pending_cleanup.retention_days;
        let cleanup_schedule = mimir_core::job_queue::DailySchedule::parse(
            &cfg.knowledge.pending_cleanup.schedule_time,
        )?;
        let cleanup_job = Job::new(
            "knowledge.pending_cleanup",
            JobPriority::System,
            Some(cleanup_schedule),
            true,
            move |_ctx: JobContext| {
                let kg = Arc::clone(&kg_for_cleanup);
                let retention = cleanup_retention_days;
                Box::pin(async move {
                    let deleted = kg
                        .delete_stale_pending(retention)
                        .await
                        .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                    if deleted > 0 {
                        tracing::info!(
                            "knowledge.pending_cleanup: deleted {deleted} stale pending fact(s)"
                        );
                    }
                    Ok(())
                })
            },
        );
        job_queue.register(cleanup_job).await?;

        // Register the events & reminders upcoming-scan job (issue #74).
        // One scheduled job per configured run time; each shares the same
        // deterministic scan handler (derive overlays + auto-complete +
        // recurring advancement).
        let kg_for_events = Arc::clone(&knowledge_graph);
        let events_horizon = cfg.knowledge.events.horizon_days as i64;
        for (idx, time_str) in cfg.knowledge.events.schedule_times.iter().enumerate() {
            let events_schedule = mimir_core::job_queue::DailySchedule::parse(time_str)?;
            let job_id = format!("events.upcoming_scan_{idx}");
            let kg_clone = Arc::clone(&kg_for_events);
            let horizon = events_horizon;
            let events_job = Job::new(
                job_id,
                JobPriority::System,
                Some(events_schedule),
                true,
                move |_ctx: JobContext| {
                    let kg = Arc::clone(&kg_clone);
                    let horizon = horizon;
                    Box::pin(async move {
                        let summary = kg
                            .run_events_scan(horizon)
                            .await
                            .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                        if summary.derived + summary.completed + summary.advanced > 0 {
                            tracing::info!(
                                "events.upcoming_scan: derived {} completed {} advanced {}",
                                summary.derived,
                                summary.completed,
                                summary.advanced
                            );
                        }
                        Ok(())
                    })
                },
            );
            job_queue.register(events_job).await?;
        }

        Ok((
            Self {
                llm_client,
                context_manager,
                config,
                session_locks: Arc::new(DashMap::new()),
                start_time: Instant::now(),
                endpoint: cfg.llm.endpoint.clone(),
                model: cfg.llm.model.clone(),
                shutdown_tx,
                model_override_cache: Arc::new(DashMap::new()),
                tool_registry,
                knowledge_graph,
                job_queue,
                agent_runtime,
                scheduler,
                last_user_activity,
                user_entity_id,
            },
            scheduler_shutdown_rx,
        ))
    }

    /// Record the current time as the most recent user interaction.
    pub fn record_user_activity(&self) {
        self.last_user_activity
            .store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        self.scheduler.notify_user_activity();
    }

    /// Return (or create) the semaphore for a given session id.
    pub fn session_semaphore(&self, session_id: i64) -> Arc<tokio::sync::Semaphore> {
        self.session_locks
            .entry(session_id)
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
    }

    /// Gracefully shut down all long-lived resources.
    ///
    /// 1. Stop the background scheduler (no new ingestion enqueues overlays).
    /// 2. Drain pending location-overlay jobs so queued `entity_locations`
    ///    upserts complete before resources are torn down.
    /// 3. Shut down the LLM worker pool and drop HTTP clients.
    /// 4. Signal completion.
    pub async fn shutdown(&self) {
        tracing::info!("Shutting down scheduler...");
        self.scheduler.shutdown();

        tracing::info!("Draining pending location-overlay jobs...");
        self.knowledge_graph.flush_location_overlays().await;

        tracing::info!("Shutting down ContextManager...");
        self.context_manager.close().await;

        tracing::info!("Shutting down LLM client...");
        self.llm_client.shutdown().await;

        tracing::info!("Shutdown complete.");
    }

    /// Resolve the LLM backend to use, applying a model override if requested.
    ///
    /// Override clients are cached so that repeated requests with the same
    /// model do not allocate a new [`Arc`] on every call.
    pub fn resolve_llm(&self, model_override: Option<String>) -> Arc<dyn LlmBackend> {
        let model = match model_override {
            Some(m) => m,
            None => return Arc::clone(&self.llm_client),
        };
        if let Some(cached) = self.model_override_cache.get(&model) {
            return cached.clone();
        }
        let client = self
            .llm_client
            .with_model_override(model.clone())
            .unwrap_or_else(|| Arc::clone(&self.llm_client));

        // Evict an arbitrary entry if at capacity so memory cannot grow without bound.
        if self.model_override_cache.len() >= MODEL_OVERRIDE_CACHE_CAP
            && let Some(entry) = self.model_override_cache.iter().next()
        {
            let key = entry.key().clone();
            drop(entry);
            self.model_override_cache.remove(&key);
        }

        self.model_override_cache.insert(model, Arc::clone(&client));
        client
    }
}

/// Insert name/preferred-name facts for the user entity if they do not already exist.
/// Facts are categorised as Identity (110) so they appear in the identity bucket of memory.
pub(crate) async fn seed_identity_facts(
    kg: &mimir_knowledge::KnowledgeGraph,
    subject_id: i32,
    name: &str,
    preferred: &str,
) -> Result<(), mimir_knowledge::KnowledgeError> {
    use mimir_knowledge::models::fact::FactStatus;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::SourceType;

    // Resolve predicate IDs via the cached registry.
    let has_name_id = kg.ensure_relationship_type("has_name").await?;
    let pref_name_id = kg.ensure_relationship_type("preferred_name").await?;

    // Targeted existence checks: query only the two relevant predicates.
    let has_name_facts = kg
        .get_facts_by_subject_and_predicate(subject_id, has_name_id)
        .await?;
    let pref_name_facts = kg
        .get_facts_by_subject_and_predicate(subject_id, pref_name_id)
        .await?;

    let has_name = has_name_facts.iter().any(|f| {
        f.status() == Some(FactStatus::Active)
            && f.object_literal
                .as_deref()
                .map(|lit| lit.to_lowercase() == name.to_lowercase())
                .unwrap_or(false)
    });
    let has_preferred = pref_name_facts.iter().any(|f| {
        f.status() == Some(FactStatus::Active)
            && f.object_literal
                .as_deref()
                .map(|lit| lit.to_lowercase() == preferred.to_lowercase())
                .unwrap_or(false)
    });

    // Collect facts to insert and perform the writes atomically.
    // Insert identity facts *before* alias/auto-merge so the canonical entity
    // always has at least as many facts as any qualifying duplicate, ensuring
    // auto_merge_pair preserves subject_id as the survivor.
    let mut facts_to_insert: Vec<NewFact> = Vec::with_capacity(2);

    if !has_name && !name.is_empty() {
        let mut nf = NewFact::new(subject_id, "has_name");
        nf.object_literal = Some(name.to_string());
        nf.source_type = SourceType::System;
        nf.category_ids = vec![110];
        facts_to_insert.push(nf);
    }

    if !preferred.is_empty() && preferred.to_lowercase() != name.to_lowercase() && !has_preferred {
        let mut nf = NewFact::new(subject_id, "preferred_name");
        nf.object_literal = Some(preferred.to_string());
        nf.source_type = SourceType::System;
        nf.category_ids = vec![110];
        facts_to_insert.push(nf);
    }

    if !facts_to_insert.is_empty() {
        kg.insert_facts_batch(facts_to_insert).await?;
    }

    // Alias logic (idempotent; safe to run outside the insert tx).
    if !preferred.is_empty() && preferred.to_lowercase() != name.to_lowercase() {
        warn_err(
            kg.add_alias(subject_id, preferred).await,
            &format!("Failed to add preferred-name alias '{preferred}'"),
        );

        auto_merge_accidental_duplicates(kg, subject_id, preferred).await;
    }

    Ok(())
}

/// Merge bare-name duplicate entities that look accidental (very few facts).
///
/// A threshold of 2 was chosen because a legitimate entity should have at least
/// a name fact and a preferred-name fact; anything less suggests an accidental
/// duplicate created before the alias was wired.
async fn auto_merge_accidental_duplicates(
    kg: &mimir_knowledge::KnowledgeGraph,
    subject_id: i32,
    preferred: &str,
) {
    let candidates = warn_err(
        mimir_knowledge::queries::entity::get_by_name(kg.pool(), preferred).await,
        &format!("Failed to look up duplicates of '{preferred}'"),
    )
    .unwrap_or_default();

    for cand in candidates {
        try_merge_accidental_duplicate(kg, subject_id, preferred, cand).await;
    }
}

/// Evaluate a single candidate entity and merge it into `subject_id` if it looks
/// like an accidental duplicate (very few facts and same name).
async fn try_merge_accidental_duplicate(
    kg: &mimir_knowledge::KnowledgeGraph,
    subject_id: i32,
    preferred: &str,
    cand: mimir_knowledge::queries::entity::AliasSearchResult,
) {
    if cand.entity.id == subject_id || cand.entity.name.to_lowercase() != preferred.to_lowercase() {
        return;
    }

    let fact_count = warn_err(
        kg.count_entity_facts(cand.entity.id).await,
        &format!(
            "Failed to count facts for candidate entity {} during auto-merge check",
            cand.entity.id
        ),
    )
    .unwrap_or(i64::MAX);

    if fact_count > ACCIDENTAL_DUPLICATE_FACT_THRESHOLD {
        return;
    }

    warn_err(
        mimir_knowledge::queries::entity::auto_merge_pair(kg.pool(), subject_id, cand.entity.id)
            .await,
        &format!(
            "Failed to auto-merge duplicate entity {} into {}",
            cand.entity.id, subject_id
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::warn_err;

    #[test]
    fn warn_err_returns_some_on_ok() {
        assert_eq!(
            warn_err::<i32, std::io::Error>(Ok(42), "expected success"),
            Some(42)
        );
    }

    #[test]
    fn warn_err_returns_none_on_err() {
        let err = std::io::Error::other("boom");
        assert!(warn_err::<i32, std::io::Error>(Err(err), "expected failure").is_none());
    }
}
