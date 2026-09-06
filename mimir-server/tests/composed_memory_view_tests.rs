mod common;

use common::*;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_server::memory_view::{BudgetPolicy, compose_memory_view};

#[tokio::test]
async fn composed_view_exposes_core_upcoming_and_budget_metadata() {
    let mut config = Config::default();
    config.identity.name = "Devansh".to_string();
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state_with_config(mock, config).await;
    state
        .knowledge_graph
        .set_condensed_memory("Stable memory facts.")
        .await
        .unwrap();

    let view = compose_memory_view(&state).await;

    assert_eq!(view.core.as_deref(), Some("Stable memory facts."));
    assert!(view.core_available);
    assert!(!view.core_degraded);
    assert!(view.upcoming_available);
    assert!(!view.upcoming_degraded);
    assert_eq!(view.temporal_horizon_days, 30);
    assert_eq!(view.char_limit, 2500);
    assert_eq!(view.usage.char_count, view.content().chars().count());
    assert!(view.warnings.is_empty());
}

#[tokio::test]
async fn composed_view_budgeted_render_keeps_core_within_char_limit() {
    let mut config = Config::default();
    config.identity.name = "Devansh".to_string();
    config.memory.char_limit = 80;
    config.memory.temporal_horizon = 7;
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state_with_config(mock, config).await;
    let user_id = state
        .user_entity_id
        .expect("identity configuration resolves a user entity");
    let mut fact = NewFact::new(user_id, "has_event");
    fact.object_literal = Some("Review memory architecture".to_string());
    fact.valid_from = Some(state.knowledge_graph.now() + chrono::Duration::days(1));
    fact.source_type = SourceType::UserEdit;
    fact.category_ids = vec![930];
    fact.confidence = Some(0.9);
    state.knowledge_graph.insert_fact(fact).await.unwrap();
    state
        .knowledge_graph
        .set_condensed_memory("Core memory that must not be dropped.")
        .await
        .unwrap();

    let view = compose_memory_view(&state).await;
    let prompt_memory = view.render(BudgetPolicy::Budgeted);

    assert!(prompt_memory.contains("Core memory that must not be dropped."));
    assert!(prompt_memory.contains("Upcoming:"));
    assert!(prompt_memory.chars().count() <= 80);
    assert!(!prompt_memory.starts_with("Now: "));
}

#[tokio::test]
async fn composed_view_reports_disabled_and_unresolved_memory_as_degraded() {
    let mut config = Config::default();
    config.identity.name = String::new();
    config.memory.enabled = false;
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state_with_config(mock, config).await;

    let view = compose_memory_view(&state).await;

    assert!(!view.core_available);
    assert!(!view.upcoming_available);
    assert!(view.upcoming_degraded);
    assert_eq!(
        view.states.status,
        mimir_server::memory_view::MemoryStatusState::Degraded
    );
    assert!(
        view.warnings
            .iter()
            .any(|warning| warning == "Memory is disabled.")
    );
    assert!(
        view.warnings
            .iter()
            .any(|warning| warning == "No user identity is configured for upcoming memory.")
    );
}

#[tokio::test]
async fn memory_route_uses_the_shared_composed_view() {
    let mut config = Config::default();
    config.identity.name = "Devansh".to_string();
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state_with_config(mock, config).await;
    state
        .knowledge_graph
        .set_condensed_memory("Shared memory view content.")
        .await
        .unwrap();
    let view = compose_memory_view(&state).await;

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(authed_request().uri("/memory").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert_eq!(text, view.render(BudgetPolicy::Full));
}
