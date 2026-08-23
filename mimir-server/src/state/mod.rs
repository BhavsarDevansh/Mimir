//! Shared application state for the HTTP server.
//!
//! [`AppState`] owns every long-lived daemon resource (LLM client, knowledge
//! graph, scheduler, connector supervisor). Construction lives in
//! `builder`, user-identity fact seeding in `identity`, and the public
//! surface is re-exported here so callers keep using `state::AppState`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use chrono::Utc;
use dashmap::DashMap;

use mimir_connectors::{ConnectorRegistry, ConnectorSupervisor};
use mimir_core::{
    agents::AgentRuntime, config::ReloadableConfig, context::ContextManager, hooks::HookEngine,
    job_queue::JobQueue, llm::LlmBackend, personality::PersonalityCache,
    scheduler::BackgroundScheduler, tools::ToolRegistry,
};

mod builder;
pub mod hooks;
mod identity;
#[cfg(test)]
mod tests;

pub use identity::seed_identity_facts;

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
    /// Typed background-task hooks engine (issue #386): debounced chat
    /// learning, connector item extraction, and memory condensation.
    pub hook_engine: Arc<HookEngine>,
    /// Unified background scheduler (dedupe, debounce, idle-gate).
    pub scheduler: Arc<BackgroundScheduler>,
    /// Unix timestamp (seconds) of the last user interaction. Used to yield
    /// system jobs when the user is active.
    pub last_user_activity: Arc<AtomicU64>,
    /// Cached user entity ID in the knowledge graph (resolved at startup).
    pub user_entity_id: Option<i32>,
    /// Connector factory registry: maps `(connector_type, backend)` to the
    /// factory that constructs a connector instance from its `config_json`
    /// (Phase 3 F7 / #184). Populated at startup with the built-in backends.
    pub connector_registry: Arc<ConnectorRegistry>,
    /// Supervised per-connector task lifecycle (Phase 3 F8 / #185). Owns one
    /// long-lived runner per `Active` connector instance; the daemon signals
    /// shutdown via the shared `shutdown_tx` watch channel.
    pub connector_supervisor: Arc<ConnectorSupervisor>,
    /// Local API token required on every route except `GET /health`
    /// (issue #281). Loaded (or generated) from the data dir at startup.
    pub api_token: Arc<str>,
    /// Cached personality preset registry so chat requests never re-read or
    /// re-parse preset files unless they changed (issue #453).
    pub personality_cache: Arc<PersonalityCache>,
}

const MODEL_OVERRIDE_CACHE_CAP: usize = 16;

impl AppState {
    pub fn record_user_activity(&self) {
        self.last_user_activity
            .store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        self.scheduler.notify_user_activity();
        self.hook_engine.notify_user_activity();
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

        tracing::info!("Shutting down hooks engine...");
        self.hook_engine.shutdown().await;

        // Abort every connector runner and await its termination so the
        // shared shutdown watch (fired by the caller before this method) does
        // not race the runtime teardown. The supervisor persists the last
        // completed sync cursor as part of its shutdown path.
        tracing::info!("Shutting down connector supervisor...");
        self.connector_supervisor.shutdown().await;

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
