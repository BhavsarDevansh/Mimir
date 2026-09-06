//! Shared test helpers for the mimir-server integration tests.
//! Re-exports are used through `use common::*;` in each test binary, so unused-import
//! warnings here are expected per-binary.
#![allow(unused_imports)]

pub use std::sync::Arc;
pub use std::time::Duration;
pub use std::time::Instant;

pub use axum::body::Body;
pub use axum::http::{Request, StatusCode};
pub use dashmap::DashMap;
pub use tower::ServiceExt;

/// Assert that a prompt or memory view carries a current UTC temporal anchor.
#[allow(dead_code)]
pub fn assert_current_now_stamp(content: &str) {
    assert!(
        content.starts_with("Now: "),
        "memory-bearing composition must begin with the Now stamp: {content}"
    );
    let stamp = content
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Now: "))
        .expect("memory context carries a Now stamp");
    let timestamp = stamp
        .split_whitespace()
        .next()
        .expect("Now carries a timestamp");
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp)
        .expect("Now timestamp is valid RFC 3339")
        .with_timezone(&chrono::Utc);
    let delta = (parsed - chrono::Utc::now()).abs();
    assert!(
        delta <= chrono::Duration::seconds(5),
        "Now timestamp must be current: {content}"
    );
    assert!(
        stamp.contains('(') && stamp.contains(')'),
        "Now carries weekday/date prose: {content}"
    );
}

pub use mimir_api_types::{ChatResponse, StatusResponse};
pub use mimir_core::{
    config::{Config, ReloadableConfig},
    context::ContextManager,
    job_queue::JobQueue,
    llm::types::{FunctionCall, LlmError, Message, StreamItem, ToolCall, Usage},
    llm::{LlmBackend, MockLlmClient},
};
pub use mimir_server::state::AppState;

/// The API token every test request presents (issue #281). The test
/// `AppState` is built with this exact token, so `authed_request()` requests
/// pass the auth middleware.
pub const TEST_TOKEN: &str = "test-api-token";

/// Build a request pre-authenticated with [`TEST_TOKEN`] so route tests can
/// focus on handler behaviour instead of repeating the auth header.
/// Shared fixture: not every test binary uses every helper, so dead-code
/// analysis is relaxed.
#[allow(dead_code)]
pub fn authed_request() -> axum::http::request::Builder {
    Request::builder().header("Authorization", format!("Bearer {TEST_TOKEN}"))
}

/// A loopback `ConnectInfo` extension for requests to loopback-gated routes.
/// Shared fixture: not every test binary uses every helper, so dead-code
/// analysis is relaxed for these two.
#[allow(dead_code)]
pub fn loopback_connect_info() -> axum::extract::ConnectInfo<mimir_server::LocalPeer> {
    axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        0,
    ))))
}

/// A non-loopback `ConnectInfo` extension for loopback-rejection tests.
#[allow(dead_code)]
pub fn non_loopback_connect_info() -> axum::extract::ConnectInfo<mimir_server::LocalPeer> {
    axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(std::net::SocketAddr::from((
        [192, 168, 1, 1],
        0,
    ))))
}

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
    mimir_test_support::prepare_from_template(&kg_db_path)
        .await
        .unwrap();
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
    // `retrieve_context` is registered with a factory so it flows through
    // the registry like every other tool, rebuilt per request with the
    // request-resolved LLM (issue #441). Mirrors production wiring in
    // `state/builder.rs`.
    let retrieve_kg = Arc::clone(&knowledge_graph);
    let retrieve_context_manager = Arc::clone(&context_manager);
    tool_registry
        .register_native_with_factory(
            Arc::new(mimir_knowledge::RetrieveContextTool::new(
                Arc::clone(&knowledge_graph),
                Arc::clone(&context_manager),
                Arc::clone(&llm),
            )),
            Arc::new(move |ctx: &mimir_core::tools::ToolContext| {
                let mut tool = mimir_knowledge::RetrieveContextTool::new(
                    Arc::clone(&retrieve_kg),
                    Arc::clone(&retrieve_context_manager),
                    Arc::clone(&ctx.llm),
                );
                if let Some(ref tx) = ctx.progress {
                    tool = tool.with_progress(tx.clone());
                }
                Arc::new(tool)
            }),
        )
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

    // Hooks engine (issue #386): register the `remember.chat` hook and run
    // the dispatch loop so chat-learning tests exercise the full path.
    // Tests that need the condensation hook register it themselves.
    let (hook_engine, hook_shutdown_rx) =
        mimir_core::hooks::HookEngine::new(Arc::clone(&job_queue), Arc::clone(&llm));
    hook_engine
        .register(mimir_core::hooks::Hook {
            id: "remember.chat".to_string(),
            trigger: mimir_core::hooks::TriggerKind::TurnCompleted,
            key_scope: mimir_core::hooks::KeyScope::PerKey,
            policy: mimir_core::hooks::QueuePolicy::SingularLastWins {
                debounce: std::time::Duration::from_secs(
                    config.agent.remember_debounce_seconds as u64,
                ),
            },
            gate: mimir_core::hooks::Gate::IdleGated {
                cooldown: std::time::Duration::from_secs(config.scheduler.cooldown_seconds as u64),
            },
            retry: mimir_core::hooks::RetryPolicy::default(),
            max_pending: None,
            merge: Some(mimir_server::state::hooks::merge_chat_turns),
            handler: Arc::new(mimir_server::state::hooks::ChatLearningHandler::new(
                Arc::clone(&knowledge_graph),
                Arc::clone(&llm),
            )),
        })
        .await
        .unwrap();
    let hook_engine_clone = Arc::clone(&hook_engine);
    tokio::spawn(async move { hook_engine_clone.start(hook_shutdown_rx).await });

    // Librarian registered to mirror production, though tests no longer
    // auto-invoke it (issue #137): learning is hook-driven via
    // `remember.chat` (issue #386). Kept so the on-demand library API stays
    // exercised.
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
            mimir_knowledge::models::enums::ConnectorType::Email,
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
        hook_engine,
        scheduler,
        user_entity_id,
        last_user_activity,
        connector_registry,
        connector_supervisor,
        api_token: Arc::from(TEST_TOKEN),
        personality_cache: Arc::new(mimir_core::personality::PersonalityCache::default()),
    });

    (state, temp)
}

// Shared fixture: not every test binary uses this helper, so dead-code
// analysis is relaxed.
#[allow(dead_code)]
pub async fn test_state(llm: Arc<dyn LlmBackend>) -> (Arc<AppState>, tempfile::TempDir) {
    test_state_with_config(llm, Config::default()).await
}

// ---------------------------------------------------------------------------
// Learning-hook fixtures (issue #386): shared by the native chat and
// OpenAI-compatible provider suites. Not every test binary uses every
// helper, so dead-code analysis is relaxed for the whole block.
// ---------------------------------------------------------------------------

/// Config with zero debounce/cooldown so the `remember.chat` hook dispatches
/// immediately after the turn instead of waiting for the production windows.
#[allow(dead_code)]
pub fn fast_learning_config() -> Config {
    let mut config = Config::default();
    config.identity.name = "Devansh".to_string();
    config.agent.remember_debounce_seconds = 0;
    config.scheduler.cooldown_seconds = 0;
    config
}

/// The extraction response the `remember.chat` hook's Librarian pipeline
/// consumes: a `remember` tool call carrying one fact.
#[allow(dead_code)]
pub fn extraction_message() -> Message {
    let remember_output = mimir_knowledge::extract::RememberOutput {
        facts: vec![mimir_knowledge::extract::ExtractedFact {
            classification: mimir_knowledge::extract::Classification::Explicit,
            subject: "Devansh".to_string(),
            subject_type: "Person".to_string(),
            relationship_type: "prefers".to_string(),
            object: "blue".to_string(),
            object_is_entity: false,
            object_type: None,
            temporal: None,
            is_sensitive: false,
            correction_scope: None,
            categories: vec![],
            recurrence: None,
            requires_user_action: None,
            location: None,
        }],
    };
    Message {
        role: "assistant".to_string(),
        content: "".to_string(),
        tool_calls: Some(vec![ToolCall {
            index: 0,
            id: "call_remember".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: serde_json::to_string(&remember_output).unwrap(),
            },
        }]),
        tool_call_id: None,
    }
}

/// Whether the KG holds `favourite_colour=blue` for the configured user.
/// The user entity itself is created at daemon start, so persistence
/// checks must observe the *fact*, not the entity's existence.
#[allow(dead_code)]
pub async fn has_prefers_blue(state: &Arc<AppState>) -> bool {
    let search = state
        .knowledge_graph
        .search_entities("Devansh", 1)
        .await
        .unwrap();
    let Some(result) = search.first() else {
        return false;
    };
    let facts = state
        .knowledge_graph
        .get_facts_by_subject(result.entity.id, 100)
        .await
        .unwrap();
    for fact in &facts {
        let pred = state
            .knowledge_graph
            .relationship_type_name(fact.relationship_type_id)
            .await;
        if pred.as_deref() == Some("prefers") && fact.object_literal.as_deref() == Some("blue") {
            return true;
        }
    }
    false
}

/// Poll until the KG holds `favourite_colour=blue` for the user, or return
/// false after a timeout.
#[allow(dead_code)]
pub async fn wait_for_prefers_blue(state: &Arc<AppState>) -> bool {
    let state = Arc::clone(state);
    poll_until(Duration::from_secs(5), move || {
        let state = Arc::clone(&state);
        async move { has_prefers_blue(&state).await }
    })
    .await
}

/// Wait until the `remember.chat` hook has drained its pending queue, so a
/// negative persistence assertion observes a fully dispatched run (the hook
/// would have written facts by then if it fired). The wait is scoped to
/// `remember.chat` only: `running_count()` spans the whole engine, and an
/// unrelated hook such as `memory.condensation` (debounce and cooldown are
/// both zero under `fast_learning_config`) may legitimately be running and
/// would otherwise make the helper flaky.
#[allow(dead_code)]
pub async fn wait_for_chat_hook_idle(state: &Arc<AppState>) {
    let state = Arc::clone(state);
    let drained = poll_until(Duration::from_secs(5), move || {
        let state = Arc::clone(&state);
        async move { state.hook_engine.is_settled_for("remember.chat").await }
    })
    .await;
    assert!(drained, "remember.chat hook did not drain within 5s");
}

/// Poll an observable test condition on a 10 ms cadence until it becomes
/// true or the timeout expires. Returns whether the condition was observed.
#[allow(dead_code)]
pub async fn poll_until<F, Fut>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        if tokio::time::timeout(remaining, predicate())
            .await
            .unwrap_or(false)
        {
            return true;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10).min(remaining)).await;
    }
}
