mod common;
use common::*;

// -----------------------------------------------------------------
// Connector management routes (Phase 3 A1 / #202)
// -----------------------------------------------------------------
async fn connector_post(app: axum::Router, body: serde_json::Value) -> axum::response::Response {
    let body = serde_json::to_string(&body).unwrap();
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/connectors")
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .unwrap(),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn test_connector_add_list_show_remove_round_trip() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    // Add a new connector instance via the registered "test" backend.
    let resp = connector_post(
        app.clone(),
        serde_json::json!({
            "connector_type": "gmail",
            "backend": "test",
            "slug": "personal",
            "display_name": "Personal",
            "config_json": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: mimir_api_types::ConnectorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created.slug, "personal");
    assert_eq!(created.connector_type, "gmail");
    assert_eq!(created.backend, "test");
    assert_eq!(created.status, "setup");
    assert_eq!(created.auth_state, "unauthenticated");
    assert_eq!(created.item_count, 0);
    let id = created.id;

    // GET /connectors lists the instance.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/connectors")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: mimir_api_types::ConnectorListResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.connectors.len(), 1);
    assert_eq!(list.connectors[0].id, id);

    // GET /connectors/{id} shows it.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/connectors/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // DELETE /connectors/{id} removes it.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/connectors/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET /connectors/{id} now 404s.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/connectors/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_connector_add_rejects_existing_slug() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let body = serde_json::json!({
        "connector_type": "gmail",
        "backend": "test",
        "slug": "dupe",
        "display_name": "Dupe",
        "config_json": {},
    });
    let resp = connector_post(app.clone(), body.clone()).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = connector_post(app, body).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
/// Two concurrent `POST /connectors` for the same slug: exactly one wins
/// (`201`), the other gets `409 Conflict` — the atomic create-only insert
/// closes the read-then-write window the pre-read plus upsert had
/// (#202 review).
#[tokio::test]
async fn test_connector_add_concurrent_same_slug_one_wins() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let body = serde_json::json!({
        "connector_type": "gmail",
        "backend": "test",
        "slug": "race",
        "display_name": "Race",
        "config_json": {},
    });
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let app_a = app.clone();
    let app_b = app.clone();
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();
    let body_a = body.clone();
    let body_b = body.clone();

    let a = tokio::spawn(async move {
        barrier_a.wait().await;
        connector_post(app_a, body_a).await
    });
    let b = tokio::spawn(async move {
        barrier_b.wait().await;
        connector_post(app_b, body_b).await
    });

    let ra = a.await.unwrap();
    let rb = b.await.unwrap();
    let wins = [ra.status(), rb.status()]
        .iter()
        .filter(|&&c| c == StatusCode::CREATED)
        .count();
    let conflicts = [ra.status(), rb.status()]
        .iter()
        .filter(|&&c| c == StatusCode::CONFLICT)
        .count();
    assert_eq!(wins, 1, "exactly one concurrent POST must return 201");
    assert_eq!(
        conflicts, 1,
        "the losing concurrent POST must return 409 Conflict"
    );
}
#[tokio::test]
async fn test_connector_add_rejects_unregistered_backend() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_post(
        app,
        serde_json::json!({
            "connector_type": "gmail",
            "backend": "no-such-backend",
            "slug": "x",
            "display_name": "X",
            "config_json": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn test_connector_add_rejects_unknown_type() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_post(
        app,
        serde_json::json!({
            "connector_type": "rss",
            "backend": "test",
            "slug": "x",
            "display_name": "X",
            "config_json": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn test_connector_round_trip_via_mimir_client() {
    // Acceptance criterion for #202: list/status/add/remove round-trip
    // via `mimir-client` over a real TCP listener (not just `oneshot`).
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = mimir_client::MimirClient::new(format!("http://127.0.0.1:{port}"));
    let req = mimir_api_types::AddConnectorRequest {
        connector_type: "gmail".to_string(),
        backend: "test".to_string(),
        slug: "via-client".to_string(),
        display_name: "Via Client".to_string(),
        config_json: serde_json::json!({}),
    };
    let created = client.connector_add(req).await.unwrap();
    assert_eq!(created.slug, "via-client");
    assert_eq!(created.status, "setup");
    assert_eq!(created.item_count, 0);
    let id = created.id;

    let list = client.connectors().await.unwrap();
    assert_eq!(list.connectors.len(), 1);
    assert_eq!(list.connectors[0].id, id);

    let shown = client.connector(id).await.unwrap();
    assert_eq!(shown.backend, "test");

    client.connector_remove(id).await.unwrap();
    let list = client.connectors().await.unwrap();
    assert!(list.connectors.is_empty());
}
#[tokio::test]
async fn test_connector_remove_unknown_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/connectors/999999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("connector not found")
    );
}
/// Deleting a connector must also delete its secret-store entry so a later
/// connector created with the same slug cannot load the deleted instance's
/// credentials (#263 review).
#[tokio::test]
async fn test_connector_remove_deletes_stored_credentials() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    // Create an authenticated connector instance.
    let slug = "gmail-secret";
    let resp = connector_post(
        app.clone(),
        serde_json::json!({
            "connector_type": "gmail",
            "backend": "test",
            "slug": slug,
            "display_name": "Secret Gmail",
            "config_json": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: mimir_api_types::ConnectorResponse = serde_json::from_slice(&bytes).unwrap();
    let id = created.id;

    // Store a credential bundle keyed by the connector's slug, mimicking
    // the OAuth/app-password ingest path (A2 / #203). The test_secret
    // state injects an InMemorySecretStore, so this never touches disk.
    let secret_store = state
        .connector_supervisor
        .secret_store()
        .expect("test state injects an InMemorySecretStore");
    let bundle = mimir_connectors::SecretBundle::AppPassword {
        password: "hunter2".to_string(),
    };
    secret_store.store(slug, &bundle).await.unwrap();
    assert!(secret_store.load(slug).await.unwrap().is_some());

    // Delete the connector instance.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/connectors/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The credential must no longer be loadable.
    assert!(secret_store.load(slug).await.unwrap().is_none());

    // A new connector created with the same slug cannot load the deleted
    // instance's credentials (they are gone, not lingering).
    let resp = connector_post(
        app.clone(),
        serde_json::json!({
            "connector_type": "gmail",
            "backend": "test",
            "slug": slug,
            "display_name": "Secret Gmail 2",
            "config_json": {},
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert!(secret_store.load(slug).await.unwrap().is_none());
}
// -- Action routes (Phase 3 A2 / #203) --
/// POST to a connector sub-route (e.g. `/connectors/{id}/sync`) with an
/// optional JSON body. Attaches a loopback `ConnectInfo` so the
/// loopback-gated routes (`tokens`, `forget`) are reachable.
async fn connector_sub_post(
    app: axum::Router,
    id: i32,
    action: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let uri = format!("/connectors/{id}/{action}");
    let mut builder =
        Request::builder()
            .method("POST")
            .uri(uri)
            .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                0,
            ))));
    let req = match body {
        Some(value) => {
            let payload = serde_json::to_string(&value).unwrap();
            builder = builder.header("Content-Type", "application/json");
            builder.body(Body::from(payload)).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    app.oneshot(req).await.unwrap()
}
async fn create_test_connector(
    app: &axum::Router,
    slug: &str,
    config_json: serde_json::Value,
) -> mimir_api_types::ConnectorResponse {
    let resp = connector_post(
        app.clone(),
        serde_json::json!({
            "connector_type": "gmail",
            "backend": "test",
            "slug": slug,
            "display_name": slug,
            "config_json": config_json,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
#[tokio::test]
async fn test_connector_sync_triggers_and_returns_ok() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "sync-me", serde_json::json!({})).await;
    // Activate the connector so a sync trigger has a runner to wake.
    state.connector_supervisor.start(created.id).await.unwrap();
    // Wait for the runner to complete its auth handshake (poll, not a
    // fixed sleep, so a loaded CI runner cannot fail the trigger).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !state.connector_supervisor.is_running(created.id).await {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for connector runner to start"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let resp = connector_sub_post(app, created.id, "sync", Some(serde_json::json!({}))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["status"], "ok");
    state.connector_supervisor.stop(created.id).await;
}
#[tokio::test]
async fn test_connector_sync_unknown_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_sub_post(app, 9999, "sync", Some(serde_json::json!({}))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_connector_pause_then_resume() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "lifecycle", serde_json::json!({})).await;

    // Resume (activate) -> Active.
    let resp = connector_sub_post(app.clone(), created.id, "resume", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: mimir_api_types::ConnectorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.status, "active");

    // Pause -> Paused.
    let resp = connector_sub_post(app.clone(), created.id, "pause", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: mimir_api_types::ConnectorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.status, "paused");
    state.connector_supervisor.stop(created.id).await;
}
#[tokio::test]
async fn test_connector_pause_unknown_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_sub_post(app, 9999, "pause", None).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_connector_tokens_ingest_flips_auth_state() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "oauth-me", serde_json::json!({})).await;
    assert_eq!(created.auth_state, "unauthenticated");

    let resp = connector_sub_post(
        app,
        created.id,
        "tokens",
        Some(serde_json::json!({
            "kind": "app_password",
            "password": "hunter2",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: mimir_api_types::ConnectorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.auth_state, "authenticated");

    // The secret is stored keyed by slug.
    let secret_store = state.connector_supervisor.secret_store().unwrap();
    let loaded = secret_store.load("oauth-me").await.unwrap();
    assert!(loaded.is_some());
}
#[tokio::test]
async fn test_connector_tokens_unknown_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_sub_post(
        app,
        9999,
        "tokens",
        Some(serde_json::json!({"kind": "app_password", "password": "x"})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_connector_tokens_bad_expiry_returns_400() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "bad-exp", serde_json::json!({})).await;
    let resp = connector_sub_post(
        app,
        created.id,
        "tokens",
        Some(serde_json::json!({
            "kind": "oauth",
            "access_token": "at",
            "expires_at": "not-a-date",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn test_connector_actions_dispatch() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created =
        create_test_connector(&app, "actions-me", serde_json::json!({"act_kind": "echo"})).await;

    let resp = connector_sub_post(
        app,
        created.id,
        "actions",
        Some(serde_json::json!({
            "kind": "echo",
            "payload": {"native_id": "n1", "message": "hi"},
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: mimir_api_types::ActionResultResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(body.success);
    assert_eq!(body.native_id.as_deref(), Some("n1"));
    assert_eq!(body.message.as_deref(), Some("hi"));
}
#[tokio::test]
async fn test_connector_actions_unsupported_returns_400() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "no-act", serde_json::json!({})).await;
    let resp = connector_sub_post(
        app,
        created.id,
        "actions",
        Some(serde_json::json!({"kind": "bogus", "payload": {}})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn test_connector_actions_unknown_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_sub_post(
        app,
        9999,
        "actions",
        Some(serde_json::json!({"kind": "echo", "payload": {}})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_connector_forget_cascade_trashes_facts_and_removes_row() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "forget-me", serde_json::json!({})).await;

    // Store a credential so the cascade's secret deletion is exercised.
    let secret_store = state.connector_supervisor.secret_store().unwrap();
    secret_store
        .store(
            &created.slug,
            &mimir_connectors::SecretBundle::ApiToken {
                token: "tok".to_string(),
            },
        )
        .await
        .unwrap();

    // Insert a connector-sourced fact directly via the KG.
    use mimir_knowledge::models::audit_log::ChangedBy;
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::SourceType;
    use mimir_knowledge::queries::source::AddSourceRequest;
    let entity = state
        .knowledge_graph
        .create_entity("Forget-Target", EntityType::Concept, &[])
        .await
        .unwrap();
    let mut nf = NewFact::new(entity.id, "has_name");
    nf.object_literal = Some("val".to_string());
    let fact = state.knowledge_graph.insert_fact(nf).await.unwrap();
    state
        .knowledge_graph
        .add_source_to_fact(AddSourceRequest {
            fact_id: fact.id,
            source_type: SourceType::Connector,
            connector_instance_id: Some(created.id),
            connector_type: Some(mimir_knowledge::models::enums::ConnectorType::Gmail),
            raw_reference: Some("raw-1".to_string()),
            extraction_method: None,
            changed_by: ChangedBy::System,
        })
        .await
        .unwrap();
    assert_eq!(
        state
            .knowledge_graph
            .count_sources_for_connector(created.id)
            .await
            .unwrap(),
        1
    );

    let resp = connector_sub_post(app, created.id, "forget", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: mimir_api_types::ForgetConnectorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.forgotten_count, 1);

    // The connector row is gone.
    assert!(
        state
            .knowledge_graph
            .get_connector(created.id)
            .await
            .unwrap()
            .is_none()
    );
    // The fact is trashed (no longer active).
    assert!(
        state
            .knowledge_graph
            .get_fact(fact.id)
            .await
            .unwrap()
            .is_none()
    );
    // The stored credential is gone.
    assert!(secret_store.load(&created.slug).await.unwrap().is_none());
}
#[tokio::test]
async fn test_connector_forget_unknown_returns_404() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let resp = connector_sub_post(app, 9999, "forget", None).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
/// The credential-ingest and forget routes are loopback-only: a
/// non-loopback caller must be rejected before any mutation.
#[tokio::test]
async fn test_connector_tokens_and_forget_reject_non_loopback() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());
    let created = create_test_connector(&app, "guarded", serde_json::json!({})).await;

    for action in ["tokens", "forget"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/connectors/{}/{}", created.id, action))
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "non-loopback {action} must be rejected"
        );
    }

    // The connector row is untouched by the rejected requests.
    assert!(
        state
            .knowledge_graph
            .get_connector(created.id)
            .await
            .unwrap()
            .is_some()
    );
}
