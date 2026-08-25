use super::builder::{
    init_connector_framework, init_hook_engine, init_knowledge_graph, init_scheduler,
    optimization_resource_limits,
};
use super::hooks::{ChatLearningHandler, merge_chat_turns};
use super::warn_err;
use mimir_core::config::Config;
use mimir_core::conversation::ConversationTurn;
use mimir_core::hooks::{HookContext, HookHandler, HookOutcome};
use mimir_core::llm::MockLlmClient;
use mimir_core::llm::types::Usage;
use mimir_core::tools::ToolRegistry;
use std::sync::Arc;

fn test_config(temp: &tempfile::TempDir) -> Config {
    let mut config = Config::default();
    config.identity.name = "Test User".to_string();
    config.identity.preferred_name = "Test".to_string();
    config.context.db_path = Some(temp.path().join("context.db"));
    config.knowledge.db_path = Some(temp.path().join("knowledge.db"));
    config.scheduler.db_path = Some(temp.path().join("jobs.db"));
    config
}

fn test_llm() -> Arc<dyn mimir_core::llm::LlmBackend> {
    Arc::new(MockLlmClient::builder().build())
}

async fn test_kg(temp: &tempfile::TempDir) -> Arc<mimir_knowledge::KnowledgeGraph> {
    Arc::new(
        mimir_knowledge::KnowledgeGraph::init(&temp.path().join("knowledge.db"))
            .await
            .unwrap(),
    )
}

/// Shared inputs for [`init_knowledge_graph`] tests: an empty tool registry, an
/// isolated context manager, and a mock LLM backend.
async fn kg_init_inputs(
    temp: &tempfile::TempDir,
) -> (
    Arc<ToolRegistry>,
    Arc<mimir_core::context::ContextManager>,
    Arc<dyn mimir_core::llm::LlmBackend>,
) {
    let context_manager = Arc::new(
        mimir_core::context::ContextManager::new(&temp.path().join("context.db"))
            .await
            .unwrap(),
    );
    (Arc::new(ToolRegistry::new()), context_manager, test_llm())
}

#[cfg(not(feature = "secrets-keyring"))]
#[test]
fn keychain_backend_without_feature_fails_loudly() {
    let mut config = Config::default();
    config.secrets.backend = mimir_core::config::SecretsBackend::Keychain;
    let err = super::builder::build_secret_store(&config).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("secrets-keyring"),
        "error must point at the missing cargo feature: {message}"
    );
    assert!(
        message.contains("keychain"),
        "error must name the configured backend: {message}"
    );
}

#[cfg(all(
    feature = "secrets-keyring",
    any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "macos",
        target_os = "windows"
    )
))]
#[test]
fn keychain_backend_with_feature_constructs_os_store() {
    let mut config = Config::default();
    config.secrets.backend = mimir_core::config::SecretsBackend::Keychain;
    let store = super::builder::build_secret_store(&config).unwrap();
    assert!(
        store.is_some(),
        "keychain backend must construct the OS-keychain store"
    );
}

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

#[tokio::test]
async fn init_knowledge_graph_resolves_user_entity_and_registers_kg_tools() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp);
    let (tool_registry, context_manager, llm) = kg_init_inputs(&temp).await;

    let init = init_knowledge_graph(&config, &tool_registry, &context_manager, &llm)
        .await
        .unwrap();

    let user_id = init.user_entity_id.expect("user entity resolved");
    let facts = init
        .knowledge_graph
        .get_facts_by_subject(user_id, 100)
        .await
        .unwrap();
    assert!(
        facts
            .iter()
            .any(|f| f.object_literal.as_deref() == Some("Test User")),
        "identity has_name fact seeded"
    );
    assert!(tool_registry.get("kg_query").is_some());
    assert!(
        tool_registry.get("remember").is_none(),
        "the remember tool is removed (issue #386): learning is hook-driven"
    );
    // The builder treats geocoder construction as best-effort: it is disabled
    // when the Nominatim HTTP client or rate limiter cannot be built, so the
    // geocoder may legitimately be absent on some hosts. When present, it
    // must be the instance shared with the knowledge graph.
    if let Some(geocoder) = &init.geocoder {
        assert!(
            init.knowledge_graph
                .geocoder()
                .is_some_and(|kg_geocoder| Arc::ptr_eq(geocoder, kg_geocoder)),
            "geocoder shared with knowledge graph"
        );
    }
    assert!(init.backup_dir.ends_with("backups"));
}

#[tokio::test]
async fn chat_learning_handler_returns_retryable_failure_on_extraction_error() {
    // Issue #386: a transient extraction failure must not drop the
    // accumulated transcript — the hook's retry policy re-enqueues the
    // instance so the burst is re-extracted.
    let temp = tempfile::tempdir().unwrap();
    let kg = test_kg(&temp).await;
    let llm: Arc<dyn mimir_core::llm::LlmBackend> = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .build(),
    );
    let handler = ChatLearningHandler::new(Arc::clone(&kg), Arc::clone(&llm));
    let outcome = handler
        .run(
            Arc::new(vec![ConversationTurn::new(
                1,
                "My favourite colour is blue",
                "Noted.",
            )]),
            HookContext {
                attempt: 1,
                max_attempts: 3,
                cancellation_token: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await;
    assert_eq!(
        outcome,
        HookOutcome::RetryableFailure,
        "a transient extraction failure must be retried, not dropped"
    );
}

#[test]
fn merge_chat_turns_keeps_accumulation_when_new_payload_type_is_unexpected() {
    // A malformed trigger payload must not discard the accumulated
    // transcript: only the bad payload is lost, and the next valid turn
    // still extracts everything accumulated so far (issue #386 review).
    let turns = vec![ConversationTurn::new(1, "hello", "hi")];
    let old: Arc<dyn std::any::Any + Send + Sync> = Arc::new(turns.clone());
    let unexpected: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());

    let merged = merge_chat_turns(old, unexpected);
    let merged_turns = merged
        .downcast_ref::<Vec<ConversationTurn>>()
        .expect("the accumulated turns must be preserved");
    assert_eq!(merged_turns, &turns);
}

#[test]
fn merge_chat_turns_appends_valid_new_turns() {
    let old_turns = vec![ConversationTurn::new(1, "hello", "hi")];
    let new_turns = vec![ConversationTurn::new(2, "my name is Devansh", "noted")];
    let old = Arc::new(old_turns) as Arc<dyn std::any::Any + Send + Sync>;
    let new = Arc::new(new_turns) as Arc<dyn std::any::Any + Send + Sync>;

    let merged = merge_chat_turns(old, new);
    let merged_turns = merged
        .downcast_ref::<Vec<ConversationTurn>>()
        .expect("merged payload must stay a turn list");
    assert_eq!(merged_turns.len(), 2);
    assert_eq!(merged_turns[0].user_message, "hello");
    assert_eq!(merged_turns[1].user_message, "my name is Devansh");
}

#[tokio::test]
async fn init_scheduler_registers_system_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp);
    let job_queue = Arc::new(
        mimir_core::job_queue::JobQueue::init(&temp.path().join("jobs.db"))
            .await
            .unwrap(),
    );
    let knowledge_graph = test_kg(&temp).await;
    let llm = test_llm();
    let activity = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let context_manager = Arc::new(
        mimir_core::context::ContextManager::new(&temp.path().join("context.db"))
            .await
            .unwrap(),
    );
    let (hook_engine, _hook_shutdown_rx) = init_hook_engine(
        &config,
        &job_queue,
        &llm,
        &context_manager,
        &knowledge_graph,
        Some(1),
    )
    .await
    .unwrap();

    let (_scheduler, _shutdown_rx) = init_scheduler(
        &config,
        &job_queue,
        &llm,
        &knowledge_graph,
        &activity,
        &hook_engine,
        temp.path().join("backups"),
    )
    .await
    .unwrap();

    let jobs = job_queue.list_jobs().await.unwrap();
    let ids: Vec<String> = jobs.into_iter().map(|j| j.job_id).collect();
    for expected in [
        "knowledge.optimization",
        "knowledge.pending_cleanup",
        "events.upcoming_scan_0",
        "remember.chat",
        "connector_item.remember",
        "memory.condensation",
    ] {
        assert!(
            ids.iter().any(|id| id == expected),
            "missing job {expected}"
        );
    }
}

#[test]
fn optimization_resource_limits_derive_from_config() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_config(&temp);
    config.knowledge.optimization.cpu_cores = 2;
    config.knowledge.optimization.nice_level = 5;
    config.knowledge.optimization.memory_limit_mb = Some(2048);

    let limits = optimization_resource_limits(&config);
    assert_eq!(limits.cpu_cores, Some(2));
    assert_eq!(limits.nice_level, Some(5));
    assert_eq!(limits.memory_limit_bytes, Some(2048 * 1024 * 1024));

    // A zero cpu_cores value means "no affinity limit".
    config.knowledge.optimization.cpu_cores = 0;
    config.knowledge.optimization.memory_limit_mb = None;
    let limits = optimization_resource_limits(&config);
    assert_eq!(limits.cpu_cores, None);
    assert_eq!(limits.memory_limit_bytes, None);
}

#[tokio::test]
async fn init_connector_framework_registers_mock_backend() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp);
    let knowledge_graph = test_kg(&temp).await;
    let llm = test_llm();
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    let job_queue = Arc::new(
        mimir_core::job_queue::JobQueue::init(&temp.path().join("jobs.db"))
            .await
            .unwrap(),
    );
    let context_manager = Arc::new(
        mimir_core::context::ContextManager::new(&temp.path().join("context.db"))
            .await
            .unwrap(),
    );
    let (hook_engine, _hook_shutdown_rx) = init_hook_engine(
        &config,
        &job_queue,
        &llm,
        &context_manager,
        &knowledge_graph,
        Some(1),
    )
    .await
    .unwrap();

    let (registry, _supervisor) = init_connector_framework(
        &config,
        &knowledge_graph,
        &llm,
        None,
        &hook_engine,
        &shutdown_tx,
    )
    .await
    .unwrap();

    let backends = registry.backends_for(mimir_knowledge::models::enums::ConnectorType::Email);
    assert!(
        backends.iter().any(|b| b == "test"),
        "mock connector backend registered under cfg(test)"
    );
}

#[tokio::test]
async fn init_knowledge_graph_disables_geocoder_when_configured_off() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_config(&temp);
    config.geocoder.enabled = false;
    let (tool_registry, context_manager, llm) = kg_init_inputs(&temp).await;

    let init = init_knowledge_graph(&config, &tool_registry, &context_manager, &llm)
        .await
        .unwrap();

    assert!(init.geocoder.is_none(), "geocoder disabled by config");
    assert!(
        init.knowledge_graph.geocoder().is_none(),
        "knowledge graph must not hold a geocoder when disabled"
    );
}

#[tokio::test]
async fn init_knowledge_graph_enables_geocoder_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp);
    let (tool_registry, context_manager, llm) = kg_init_inputs(&temp).await;

    let init = init_knowledge_graph(&config, &tool_registry, &context_manager, &llm)
        .await
        .unwrap();

    let geocoder = init.geocoder.expect("geocoder enabled by default");
    assert!(
        init.knowledge_graph
            .geocoder()
            .is_some_and(|kg_geocoder| Arc::ptr_eq(&geocoder, kg_geocoder)),
        "geocoder shared with knowledge graph"
    );
}

#[tokio::test]
async fn session_compaction_handler_summarises_and_compacts_session() {
    // Issue #279: the hook handler drives the SessionCompactor end-to-end —
    // LLM summary stored on the session, compacted_at advanced, old turns
    // deleted.
    let temp = tempfile::tempdir().unwrap();
    let context = Arc::new(
        mimir_core::context::ContextManager::new(&temp.path().join("context.db"))
            .await
            .unwrap(),
    );
    let sid = context.create_session("sys").await.unwrap();
    for i in 0..25 {
        context
            .add_user_message(sid, format!("u{i}"))
            .await
            .unwrap();
        context
            .add_assistant_message(sid, format!("a{i}"))
            .await
            .unwrap();
    }

    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat("Summarised earlier turns", Usage::default())
            .build(),
    );
    let llm: Arc<dyn mimir_core::llm::LlmBackend> = mock.clone();
    let handler = super::hooks::SessionCompactionHandler::new(Arc::clone(&context), llm, 15);

    let outcome = handler
        .run(
            Arc::new(vec![ConversationTurn::new(sid, "hi", "hi")]),
            HookContext {
                attempt: 1,
                max_attempts: 1,
                cancellation_token: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await;

    assert_eq!(outcome, HookOutcome::Success);
    let session = context.load_session(sid).await.unwrap();
    assert_eq!(session.summary.as_deref(), Some("Summarised earlier turns"));
    assert!(session.compacted_at.is_some());
    let msgs = context.get_messages_after_compaction(sid).await.unwrap();
    assert_eq!(msgs.len(), 30, "15 retained turns remain");
    assert_eq!(msgs[0].content, "u10");
}

#[tokio::test]
async fn init_hook_engine_registers_session_compaction_when_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp);
    let job_queue = Arc::new(
        mimir_core::job_queue::JobQueue::init(&temp.path().join("jobs.db"))
            .await
            .unwrap(),
    );
    let (tool_registry, context_manager, llm) = kg_init_inputs(&temp).await;
    let kg = test_kg(&temp).await;
    let (engine, _shutdown_rx) =
        init_hook_engine(&config, &job_queue, &llm, &context_manager, &kg, None)
            .await
            .unwrap();

    engine
        .trigger(mimir_core::hooks::Trigger::TurnCompleted {
            session_id: 1,
            payload: Arc::new(vec![ConversationTurn::new(1, "hi", "hi")]),
        })
        .await;
    assert_eq!(
        engine.pending_depth_for("session.compaction").await,
        1,
        "compaction is enabled by default, so the hook must be registered"
    );
    let _ = tool_registry;
}

#[tokio::test]
async fn init_hook_engine_skips_session_compaction_when_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_config(&temp);
    config.context.compaction.enabled = false;
    let job_queue = Arc::new(
        mimir_core::job_queue::JobQueue::init(&temp.path().join("jobs.db"))
            .await
            .unwrap(),
    );
    let (_tool_registry, context_manager, llm) = kg_init_inputs(&temp).await;
    let kg = test_kg(&temp).await;
    let (engine, _shutdown_rx) =
        init_hook_engine(&config, &job_queue, &llm, &context_manager, &kg, None)
            .await
            .unwrap();

    engine
        .trigger(mimir_core::hooks::Trigger::TurnCompleted {
            session_id: 1,
            payload: Arc::new(vec![ConversationTurn::new(1, "hi", "hi")]),
        })
        .await;
    assert_eq!(
        engine.pending_depth_for("session.compaction").await,
        0,
        "a disabled compaction must not register the hook"
    );
}
