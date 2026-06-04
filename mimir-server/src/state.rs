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
    memory::loader::MemoryLoader,
    tools::ToolRegistry,
};

/// Shared application state for the HTTP server.
///
/// Holds all long-lived resources: the LLM client (backed by a worker pool),
/// the conversation context manager, memory loader, and per-session semaphores
/// to prevent concurrent mutation of the same session.
#[derive(Debug, Clone)]
pub struct AppState {
    pub llm_client: Arc<dyn LlmBackend>,
    pub context_manager: Arc<ContextManager>,
    pub memory_path: std::path::PathBuf,
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

        let memory_path = cfg
            .memory
            .path
            .clone()
            .unwrap_or_else(MemoryLoader::get_memory_path);

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let tool_registry = Arc::new(ToolRegistry::with_builtins());
        if let Some(path) = mimir_core::tools::ToolsConfig::default_path()
            && path.exists()
            && let Err(e) = tool_registry.load_tools_config(&path)
        {
            tracing::warn!("Failed to load tools config: {}", e);
        }

        // Register the memory tool with the configured path and limit.
        let memory_tool = Arc::new(mimir_core::tools::MemoryTool::new(
            memory_path.clone(),
            cfg.memory.char_limit,
        ));
        if let Err(e) = tool_registry.register_native(memory_tool) {
            tracing::warn!("Failed to register memory tool: {}", e);
        }

        // Initialise knowledge graph.
        let kg_db_path = mimir_core::paths::knowledge_db_path()?;
        let knowledge_graph = Arc::new(mimir_knowledge::KnowledgeGraph::init(&kg_db_path).await?);

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

        // Initialise job queue.
        let jobs_db_path = mimir_core::paths::jobs_db_path()?;
        let job_queue = Arc::new(JobQueue::init(&jobs_db_path).await?);
        let last_user_activity = Arc::new(AtomicU64::new(Utc::now().timestamp() as u64));

        // Register knowledge graph optimization job.
        let kg_for_job = Arc::clone(&knowledge_graph);
        let llm_for_job = Arc::clone(&llm_client);
        let activity_for_job = Arc::clone(&last_user_activity);
        let backup_dir = mimir_core::paths::data_dir()?.join("backups");
        let timeout_minutes = cfg.knowledge.optimization.timeout_minutes;
        let schedule =
            mimir_core::job_queue::DailySchedule::parse(&cfg.knowledge.optimization.schedule_time)?;

        let opt_job = Job::new(
            "knowledge.optimization",
            JobPriority::System,
            Some(schedule),
            true,
            move |_ctx: JobContext| {
                let kg = Arc::clone(&kg_for_job);
                let llm = Arc::clone(&llm_for_job);
                let activity = Arc::clone(&activity_for_job);
                let backup_dir = backup_dir.clone();
                let timeout = timeout_minutes;
                Box::pin(async move {
                    let opt_config = mimir_knowledge::optimization::OptimizationConfig {
                        backup_dir,
                        timeout_minutes: timeout,
                        schedule_time: "02:00".to_string(),
                    };
                    let runner = mimir_knowledge::optimization::OptimizationRunner::new(
                        &kg,
                        opt_config,
                        Some(llm),
                    );
                    let five_minutes = chrono::Duration::minutes(5);
                    runner
                        .run_all_with_yield(|| {
                            let last = chrono::DateTime::from_timestamp(
                                activity.load(Ordering::Relaxed) as i64,
                                0,
                            )
                            .unwrap_or_else(|| Utc::now() - chrono::Duration::days(1));
                            Utc::now() - last < five_minutes
                        })
                        .await
                        .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                    Ok(())
                })
            },
        );
        job_queue.register(opt_job).await?;

        Ok(Self {
            llm_client,
            context_manager,
            memory_path,
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
    /// 3. Sync memory.md to disk.
    pub async fn shutdown(&self) {
        tracing::info!("Shutting down ContextManager...");
        self.context_manager.close().await;

        tracing::info!("Shutting down LLM client...");
        self.llm_client.shutdown().await;

        if let Ok(file) = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&self.memory_path)
            .await
        {
            let _ = file.sync_all().await;
        }

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
