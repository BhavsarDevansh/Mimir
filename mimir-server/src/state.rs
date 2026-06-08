use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use chrono::Utc;
use dashmap::DashMap;

use mimir_core::{
    config::ReloadableConfig,
    context::ContextManager,
    job_queue::{Job, JobContext, JobPriority, JobQueue},
    llm::{LlmBackend, LlmClient},
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
    pub session_locks: Arc<DashMap<String, Arc<tokio::sync::Semaphore>>>,
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
    /// Unix timestamp (seconds) of the last user interaction. Used to yield
    /// system jobs when the user is active.
    pub last_user_activity: Arc<AtomicU64>,
    /// Cached user entity ID in the knowledge graph (resolved at startup).
    pub user_entity_id: Option<i32>,
}

const MODEL_OVERRIDE_CACHE_CAP: usize = 16;

impl AppState {
    /// Build `AppState` from the global [`ReloadableConfig`].
    pub async fn from_config(config: Arc<ReloadableConfig>) -> anyhow::Result<Self> {
        let llm_client: Arc<dyn LlmBackend> =
            Arc::new(LlmClient::new(config.snapshot().await.llm.clone()).await);
        Self::from_config_with_llm(config, llm_client).await
    }

    /// Build `AppState` from [`ReloadableConfig`] with an injected LLM backend.
    ///
    /// Primarily useful for tests that need to supply a [`MockLlmClient`]
    /// without relying on sentinel strings or config hacks.
    pub async fn from_config_with_llm(
        config: Arc<ReloadableConfig>,
        llm_client: Arc<dyn LlmBackend>,
    ) -> anyhow::Result<Self> {
        let cfg = config.snapshot().await;

        let db_path = match cfg.context.db_path.clone() {
            Some(p) => p,
            None => mimir_core::paths::default_db_path()?,
        };
        let context_manager = Arc::new(ContextManager::new(&db_path).await?);

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let tool_registry = Arc::new(ToolRegistry::with_builtins());
        if let Some(path) = mimir_core::tools::ToolsConfig::default_path()
            && path.exists()
            && let Err(e) = tool_registry.load_tools_config(&path)
        {
            tracing::warn!("Failed to load tools config: {}", e);
        }

        // Initialise knowledge graph.
        let kg_db_path = mimir_core::paths::knowledge_db_path()?;
        let knowledge_graph = Arc::new(mimir_knowledge::KnowledgeGraph::init(&kg_db_path).await?);

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

        // Register knowledge graph tools.
        if let Err(e) = tool_registry.register_native(Arc::new(mimir_knowledge::KgQueryTool::new(
            Arc::clone(&knowledge_graph),
        ))) {
            tracing::warn!("Failed to register kg_query tool: {}", e);
        }
        if let Err(e) = tool_registry.register_native(Arc::new(
            mimir_knowledge::KgRelatedTool::new(Arc::clone(&knowledge_graph)),
        )) {
            tracing::warn!("Failed to register kg_related tool: {}", e);
        }
        if let Err(e) = tool_registry.register_native(Arc::new(mimir_knowledge::KgSearchTool::new(
            Arc::clone(&knowledge_graph),
        ))) {
            tracing::warn!("Failed to register kg_search tool: {}", e);
        }
        if let Err(e) = tool_registry.register_native(Arc::new(
            mimir_knowledge::KgExpandCatalogueTool::new(Arc::clone(&knowledge_graph)),
        )) {
            tracing::warn!("Failed to register expand_catalogue tool: {}", e);
        }
        if let Err(e) = tool_registry.register_native(Arc::new(
            mimir_knowledge::KgFactsInCatalogueTool::new(Arc::clone(&knowledge_graph)),
        )) {
            tracing::warn!("Failed to register get_facts_in_catalogue tool: {}", e);
        }

        // Initialise job queue.
        let jobs_db_path = mimir_core::paths::jobs_db_path()?;
        let job_queue = Arc::new(JobQueue::init(&jobs_db_path).await?);
        let last_user_activity = Arc::new(AtomicU64::new(0));

        // Register knowledge graph optimization job.
        let kg_for_job = Arc::clone(&knowledge_graph);
        let llm_for_job = Arc::clone(&llm_client);
        let activity_for_job = Arc::clone(&last_user_activity);
        let backup_dir = mimir_core::paths::data_dir()?.join("backups");
        let timeout_minutes = cfg.knowledge.optimization.timeout_minutes;
        let schedule_time = cfg.knowledge.optimization.schedule_time.clone();
        let schedule =
            mimir_core::job_queue::DailySchedule::parse(&cfg.knowledge.optimization.schedule_time)?;

        let jq_for_opt = Arc::clone(&job_queue);
        let opt_job = Job::new(
            "knowledge.optimization",
            JobPriority::System,
            Some(schedule),
            true,
            move |_ctx: JobContext| {
                let kg = Arc::clone(&kg_for_job);
                let llm = Arc::clone(&llm_for_job);
                let activity = Arc::clone(&activity_for_job);
                let jq = Arc::clone(&jq_for_opt);
                let backup_dir = backup_dir.clone();
                let timeout = timeout_minutes;
                let schedule_time = schedule_time.clone();
                Box::pin(async move {
                    let opt_config = mimir_knowledge::optimization::OptimizationConfig {
                        backup_dir,
                        timeout_minutes: timeout,
                        schedule_time,
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
                                if let Err(e) = jq.run_now("memory.condensation").await {
                                    tracing::warn!(
                                        "Failed to trigger post-optimization condensation: {}",
                                        e
                                    );
                                }
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
                Box::pin(async move {
                    if let Some(subject_id) = uid {
                        let condenser = mimir_knowledge::condensation::MemoryCondenser::new(
                            kg, llm, subject_id, limit,
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

        Ok(Self {
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
            last_user_activity,
            user_entity_id,
        })
    }

    /// Record the current time as the most recent user interaction.
    pub fn record_user_activity(&self) {
        self.last_user_activity
            .store(Utc::now().timestamp() as u64, Ordering::Relaxed);
    }

    /// Return (or create) the semaphore for a given session id.
    pub fn session_semaphore(&self, session_id: &str) -> Arc<tokio::sync::Semaphore> {
        self.session_locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
    }

    /// Gracefully shut down all long-lived resources.
    ///
    /// 1. Close the SQLite pool (flushes WAL).
    /// 2. Shut down the LLM worker pool and drop HTTP clients.
    /// 3. Signal completion.
    pub async fn shutdown(&self) {
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
