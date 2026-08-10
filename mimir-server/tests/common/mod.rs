//! Shared test helpers for the mimir-server integration tests.
//! Re-exports are used through `use common::*;` in each test binary, so unused-import
//! warnings here are expected per-binary.
#![allow(unused_imports)]

pub use std::sync::Arc;
pub use std::time::Instant;

pub use axum::body::Body;
pub use axum::http::{Request, StatusCode};
pub use dashmap::DashMap;
pub use tower::ServiceExt;

pub use mimir_api_types::{ChatResponse, StatusResponse};
pub use mimir_core::{
    config::{Config, ReloadableConfig},
    context::ContextManager,
    job_queue::JobQueue,
    llm::types::{FunctionCall, LlmError, Message, StreamItem, ToolCall, Usage},
    llm::{LlmBackend, MockLlmClient},
};
pub use mimir_server::state::AppState;

/// Build an `AppState` suitable for tests, using a temporary directory
/// for the context database.
pub async fn test_state_with_config(
    llm: Arc<dyn LlmBackend>,
    config: Config,
) -> (Arc<AppState>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("context.db");

    let context_manager = Arc::new(ContextManager::new(&db_path).await.unwrap());
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

    let reloadable = ReloadableConfig::new(config.clone(), temp.path().join("dummy_config.toml"));

    let tool_registry = mimir_core::tools::ToolRegistry::with_builtins();

    let kg_db_path = temp.path().join("knowledge.db");
    let knowledge_graph = Arc::new(
        mimir_knowledge::KnowledgeGraph::init(&kg_db_path)
            .await
            .unwrap(),
    );
    tool_registry
        .register_native(Arc::new(mimir_knowledge::KgQueryTool::new(Arc::clone(
            &knowledge_graph,
        ))))
        .unwrap();
    tool_registry
        .register_native(Arc::new(mimir_knowledge::KgRelatedTool::new(Arc::clone(
            &knowledge_graph,
        ))))
        .unwrap();
    tool_registry
        .register_native(Arc::new(mimir_knowledge::KgSearchTool::new(Arc::clone(
            &knowledge_graph,
        ))))
        .unwrap();
    tool_registry
        .register_native(Arc::new(mimir_knowledge::KgExpandCatalogueTool::new(
            Arc::clone(&knowledge_graph),
        )))
        .unwrap();
    tool_registry
        .register_native(Arc::new(mimir_knowledge::KgFactsInCatalogueTool::new(
            Arc::clone(&knowledge_graph),
        )))
        .unwrap();
    tool_registry
        .register_native(Arc::new(mimir_knowledge::RememberTool::new(Arc::clone(
            &knowledge_graph,
        ))))
        .unwrap();

    let jobs_db_path = temp.path().join("jobs.db");
    let job_queue = Arc::new(JobQueue::init(&jobs_db_path).await.unwrap());
    let last_user_activity = Arc::new(std::sync::atomic::AtomicU64::new(
        (chrono::Utc::now() - chrono::Duration::minutes(10)).timestamp() as u64,
    ));

    // Register a dummy optimization job so the kb routes work in tests.
    let dummy_job = mimir_core::job_queue::Job::new(
        "knowledge.optimization",
        mimir_core::job_queue::JobPriority::System,
        Some(mimir_core::job_queue::DailySchedule::new(
            chrono::NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
        )),
        false,
        |_ctx: mimir_core::job_queue::JobContext| Box::pin(async move { Ok(()) }),
    );
    job_queue.register(dummy_job).await.unwrap();

    // Dummy scheduler for tests.
    let (scheduler, _sched_rx) = mimir_core::scheduler::BackgroundScheduler::new(
        Arc::clone(&job_queue),
        Arc::clone(&llm),
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    );

    // Librarian registered to mirror production, though tests no longer
    // auto-invoke it (issue #137): learning is LLM-orchestrated via
    // `remember`. Kept so the on-demand library API stays exercised.
    let agent_runtime = Arc::new(mimir_core::agents::AgentRuntime::new());
    agent_runtime
        .register::<mimir_knowledge::librarian::LibrarianAgent>(
            mimir_knowledge::librarian::LibrarianAgent::new(),
        )
        .await;

    // Resolve or create the user entity from config identity, mirroring
    // production setup, so background agents like the Librarian can run.
    let user_entity_id = if config.identity.name.trim().is_empty() {
        None
    } else {
        match knowledge_graph
            .search_entities(&config.identity.name, 1)
            .await
        {
            Ok(mut results) if !results.is_empty() => Some(results.remove(0).entity.id),
            _ => match knowledge_graph
                .create_entity(
                    &config.identity.name,
                    mimir_knowledge::models::entity::EntityType::Person,
                    &[],
                )
                .await
            {
                Ok(entity) => Some(entity.id),
                Err(e) => {
                    tracing::warn!("Failed to create test user entity: {}", e);
                    None
                }
            },
        }
    };

    // Connector registry + supervisor for the connector management routes
    // (Phase 3 A1 / #202). Only the mock factory is registered so tests
    // exercise the CRUD/status surface against a deterministic backend.
    let connector_registry = Arc::new(mimir_connectors::ConnectorRegistry::new());
    connector_registry
        .register(
            mimir_knowledge::models::enums::ConnectorType::Gmail,
            "test".to_string(),
            mimir_connectors::MockConnectorFactory,
        )
        .unwrap();
    let connector_supervisor = Arc::new(
        mimir_connectors::ConnectorSupervisor::new(
            Arc::clone(&connector_registry),
            Arc::clone(&knowledge_graph),
            mimir_connectors::SupervisorConfig::default(),
            shutdown_tx.subscribe(),
        )
        // Inject an in-memory secret store so the connector removal route
        // can exercise credential cleanup (the daemon path uses a
        // FileSecretStore; tests must not touch the real secrets dir). The
        // mock connector stores no secrets, so this is a no-op for the
        // CRUD round-trip tests and only matters for the deletion test.
        .with_secret_store(Arc::new(mimir_connectors::InMemorySecretStore::new())
            as std::sync::Arc<dyn mimir_connectors::SecretStore>)
        .with_llm_backend(Arc::clone(&llm) as std::sync::Arc<dyn LlmBackend>),
    );

    let state = Arc::new(AppState {
        llm_client: llm,
        context_manager,
        config: Arc::new(reloadable),
        session_locks: Arc::new(DashMap::new()),
        start_time: Instant::now(),
        endpoint: "http://localhost:8080".to_string(),
        model: "gpt-4o".to_string(),
        shutdown_tx,
        model_override_cache: Arc::new(DashMap::new()),
        tool_registry: Arc::new(tool_registry),
        knowledge_graph,
        job_queue,
        agent_runtime,
        scheduler,
        user_entity_id,
        last_user_activity,
        connector_registry,
        connector_supervisor,
    });

    (state, temp)
}
pub async fn test_state(llm: Arc<dyn LlmBackend>) -> (Arc<AppState>, tempfile::TempDir) {
    test_state_with_config(llm, Config::default()).await
}
