mod common;
use common::*;

/// Register a slow `memory.condensation` hook so the refresh route's
/// already-running / timeout / cancellation branches can be exercised
/// (issue #386: the route force-runs the hook through the hooks engine).
async fn register_slow_condensation_hook(state: &Arc<AppState>, sleep: std::time::Duration) {
    struct SlowCondensation {
        sleep: std::time::Duration,
    }
    #[async_trait::async_trait]
    impl mimir_core::hooks::HookHandler for SlowCondensation {
        async fn run(
            &self,
            _payload: Arc<dyn std::any::Any + Send + Sync>,
            _ctx: mimir_core::hooks::HookContext,
        ) -> mimir_core::hooks::HookOutcome {
            tokio::time::sleep(self.sleep).await;
            mimir_core::hooks::HookOutcome::Success
        }
    }
    state
        .hook_engine
        .register(mimir_core::hooks::Hook {
            id: "memory.condensation".to_string(),
            trigger: mimir_core::hooks::TriggerKind::FactInserted,
            key_scope: mimir_core::hooks::KeyScope::Global,
            policy: mimir_core::hooks::QueuePolicy::SingularLastWins {
                debounce: std::time::Duration::ZERO,
            },
            gate: mimir_core::hooks::Gate::Ungated,
            retry: mimir_core::hooks::RetryPolicy::default(),
            merge: None,
            handler: Arc::new(SlowCondensation { sleep }),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn test_memory_returns_condensed_content() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    // Seed condensed memory in the knowledge graph
    state
        .knowledge_graph
        .set_condensed_memory("Test memory content from KG.")
        .await
        .unwrap();

    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(authed_request().uri("/memory").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(text.contains("Test memory content from KG."));
}
#[tokio::test]
async fn test_memory_refresh_non_loopback_rejected() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/memory/refresh")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [192, 168, 1, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn test_memory_refresh_not_registered_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/memory/refresh")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_memory_refresh_already_running_returns_409() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    // Register a slow condensation hook so we can race it.
    register_slow_condensation_hook(&state, std::time::Duration::from_secs(2)).await;

    let app = mimir_server::build_app(Arc::clone(&state));

    // Start a force run in the background via the hooks engine directly.
    let engine = Arc::clone(&state.hook_engine);
    let _bg = tokio::spawn(async move {
        let _ = engine.force_run("memory.condensation").await;
    });

    // Give the background task a moment to mark the hook running.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/memory/refresh")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_memory_refresh_cancelled_returns_409() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    // Register a non-cooperative slow hook so the run-now cancellation branch
    // wins and the run is recorded as cancelled.
    register_slow_condensation_hook(&state, std::time::Duration::from_secs(30)).await;

    let app = mimir_server::build_app(Arc::clone(&state));
    let jq = Arc::clone(&state.job_queue);
    let response_task = tokio::spawn(async move {
        app.oneshot(
            authed_request()
                .method("POST")
                .uri("/memory/refresh")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });

    // Give the run a moment to start, then cancel it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(jq.cancel("memory.condensation"));

    let response = response_task.await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_memory_refresh_timed_out_returns_504() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    register_slow_condensation_hook(&state, std::time::Duration::from_secs(30)).await;
    state
        .job_queue
        .set_default_timeout(std::time::Duration::from_millis(100))
        .await;

    let app = mimir_server::build_app(Arc::clone(&state));
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/memory/refresh")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
}

#[tokio::test]
async fn test_memory_upcoming_respects_temporal_horizon() {
    let (state, _temp) = memory_state_with_horizon(Some(1)).await;
    let uid = state.user_entity_id.expect("user entity seeded");

    seed_future_fact(&state, uid, chrono::Duration::days(5)).await;

    let text = get_memory_body(&state).await;
    assert!(
        !text.contains("Tokyo"),
        "1-day temporal_horizon must exclude a 5-day event: {text}"
    );
}

#[tokio::test]
async fn test_memory_upcoming_includes_events_within_default_horizon() {
    let (state, _temp) = memory_state_with_horizon(None).await;
    let uid = state.user_entity_id.expect("user entity seeded");

    seed_future_fact(&state, uid, chrono::Duration::days(5)).await;

    let text = get_memory_body(&state).await;
    assert!(
        text.contains("Tokyo"),
        "default 30-day temporal_horizon must include a 5-day event: {text}"
    );
}

async fn memory_state_with_horizon(horizon: Option<u8>) -> (Arc<AppState>, tempfile::TempDir) {
    let mock = Arc::new(MockLlmClient::builder().build());
    let config = mimir_core::config::Config {
        identity: mimir_core::config::IdentityConfig {
            name: "Test User".to_string(),
            ..Default::default()
        },
        memory: mimir_core::config::MemoryConfig {
            temporal_horizon: horizon.unwrap_or(30),
            ..Default::default()
        },
        ..Default::default()
    };
    test_state_with_config(mock, config).await
}

async fn get_memory_body(state: &Arc<AppState>) -> String {
    let app = mimir_server::build_app(Arc::clone(state));
    let response = app
        .oneshot(authed_request().uri("/memory").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn seed_future_fact(state: &Arc<AppState>, uid: i32, ahead: chrono::Duration) {
    state
        .knowledge_graph
        .insert_fact(mimir_knowledge::models::fact::NewFact {
            subject_id: uid,
            relationship_type: "is_in".to_string(),
            object_id: None,
            object_literal: Some("Tokyo".to_string()),
            valid_from: Some(chrono::Utc::now() + ahead),
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
}
