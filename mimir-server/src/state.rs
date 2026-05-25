use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

use mimir_core::{
    config::Config,
    context::ContextManager,
    llm::{LlmBackend, LlmClient},
    memory::loader::MemoryLoader,
    personality::Personality,
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
    pub personality: Personality,
    /// Per-session semaphore to serialise concurrent requests for the same session.
    pub session_locks: Arc<DashMap<String, Arc<tokio::sync::Semaphore>>>,
    pub start_time: Instant,
    /// LLM endpoint URL (for status reporting).
    pub endpoint: String,
    /// Configured LLM model (for status reporting).
    pub model: String,
    /// Memory character limit (for status reporting).
    pub memory_limit: usize,
    /// Shutdown signal sender.
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Cache for model-override LLM clients to avoid allocating a new client
    /// on every request with the same override model.
    pub model_override_cache: Arc<DashMap<String, Arc<dyn LlmBackend>>>,
}

const MODEL_OVERRIDE_CACHE_CAP: usize = 16;

impl AppState {
    /// Build `AppState` from the global Mimir [`Config`].
    pub async fn from_config(config: Config) -> anyhow::Result<Self> {
        let llm_client: Arc<dyn LlmBackend> = Arc::new(LlmClient::new(config.llm.clone()).await);

        let db_path = config
            .context
            .db_path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/mimir/context.db"));
        let context_manager = Arc::new(ContextManager::new(&db_path).await?);

        let memory_path = MemoryLoader::get_memory_path();

        let personality = Personality::new(&config.personality);

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        Ok(Self {
            llm_client,
            context_manager,
            memory_path,
            personality,
            session_locks: Arc::new(DashMap::new()),
            start_time: Instant::now(),
            endpoint: config.llm.endpoint.clone(),
            model: config.llm.model.clone(),
            memory_limit: config.memory.char_limit as usize,
            shutdown_tx,
            model_override_cache: Arc::new(DashMap::new()),
        })
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
