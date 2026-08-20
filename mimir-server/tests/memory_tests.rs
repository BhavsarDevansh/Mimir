mod common;
use common::*;

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

    // Register a slow condensation job so we can race it.
    let slow_job = mimir_core::job_queue::Job::new(
        "memory.condensation",
        mimir_core::job_queue::JobPriority::System,
        None,
        true,
        |_ctx: mimir_core::job_queue::JobContext| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                Ok(())
            })
        },
    );
    state.job_queue.register(slow_job).await.unwrap();

    let app = mimir_server::build_app(Arc::clone(&state));

    // Start a run in the background via the job queue directly.
    let jq = Arc::clone(&state.job_queue);
    let _bg = tokio::spawn(async move {
        let _ = jq.run_now("memory.condensation").await;
    });

    // Give the background task a moment to insert the Running row.
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

    // Register a non-cooperative slow job so the run-now cancellation branch
    // wins and the run is recorded as cancelled.
    let slow_job = mimir_core::job_queue::Job::new(
        "memory.condensation",
        mimir_core::job_queue::JobPriority::System,
        None,
        true,
        |_ctx: mimir_core::job_queue::JobContext| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(())
            })
        },
    );
    state.job_queue.register(slow_job).await.unwrap();

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

    let slow_job = mimir_core::job_queue::Job::new(
        "memory.condensation",
        mimir_core::job_queue::JobPriority::System,
        None,
        true,
        |_ctx: mimir_core::job_queue::JobContext| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(())
            })
        },
    );
    state.job_queue.register(slow_job).await.unwrap();
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
