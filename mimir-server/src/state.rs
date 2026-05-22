use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

use mimir_core::{
    config::Config, context::ContextManager, llm::LlmClient, memory::loader::MemoryLoader,
    personality::Personality,
};

/// Shared application state for the HTTP server.
///
/// Holds all long-lived resources: the LLM client (backed by a worker pool),
/// the conversation context manager, memory loader, and per-session semaphores
/// to prevent concurrent mutation of the same session.
#[derive(Debug, Clone)]
pub struct AppState {
    pub llm_client: Arc<LlmClient>,
    pub context_manager: Arc<ContextManager>,
    pub memory_path: std::path::PathBuf,
    pub personality: Personality,
    /// Per-session semaphore to serialise concurrent requests for the same session.
    pub session_locks: Arc<DashMap<String, Arc<tokio::sync::Semaphore>>>,
    pub start_time: Instant,
}

impl AppState {
    /// Build `AppState` from the global Mimir [`Config`].
    pub async fn from_config(config: Config) -> anyhow::Result<Self> {
        let llm_client = Arc::new(LlmClient::new(config.llm.clone()));

        let db_path = config
            .context
            .db_path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/mimir/context.db"));
        let context_manager = Arc::new(ContextManager::new(&db_path).await?);

        let memory_path = MemoryLoader::get_memory_path();

        let personality = Personality::new(&config.personality);

        Ok(Self {
            llm_client,
            context_manager,
            memory_path,
            personality,
            session_locks: Arc::new(DashMap::new()),
            start_time: Instant::now(),
        })
    }

    /// Return (or create) the semaphore for a given session id.
    pub fn session_semaphore(&self, session_id: &str) -> Arc<tokio::sync::Semaphore> {
        self.session_locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
    }
}
