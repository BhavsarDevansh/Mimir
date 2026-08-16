use super::builder::{
    init_connector_framework, init_knowledge_graph, init_scheduler, optimization_resource_limits,
};
use super::warn_err;
use mimir_core::config::Config;
use mimir_core::llm::MockLlmClient;
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
    let tool_registry = ToolRegistry::new();
    let context_manager = Arc::new(
        mimir_core::context::ContextManager::new(&temp.path().join("context.db"))
            .await
            .unwrap(),
    );
    let llm = test_llm();

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
    assert!(tool_registry.get("remember").is_some());
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

    let (_scheduler, _shutdown_rx) = init_scheduler(
        &config,
        &job_queue,
        &llm,
        &knowledge_graph,
        &activity,
        Some(1),
        temp.path().join("backups"),
    )
    .await
    .unwrap();

    let jobs = job_queue.list_jobs().await.unwrap();
    let ids: Vec<String> = jobs.into_iter().map(|j| j.job_id).collect();
    for expected in [
        "knowledge.optimization",
        "memory.condensation",
        "knowledge.pending_cleanup",
        "events.upcoming_scan_0",
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

    let (registry, _supervisor) =
        init_connector_framework(&config, &knowledge_graph, &llm, None, &shutdown_tx)
            .await
            .unwrap();

    let backends = registry.backends_for(mimir_knowledge::models::enums::ConnectorType::Gmail);
    assert!(
        backends.iter().any(|b| b == "test"),
        "mock connector backend registered under cfg(test)"
    );
}
