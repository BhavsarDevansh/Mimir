mod common;
use common::*;

#[tokio::test]
async fn test_kg_tools_registered() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    let names: Vec<String> = state
        .tool_registry
        .list()
        .into_iter()
        .map(|m| m.name)
        .collect();

    assert!(names.contains(&"kg_query".to_string()));
    assert!(names.contains(&"kg_related".to_string()));
    assert!(names.contains(&"kg_search".to_string()));
    assert!(names.contains(&"expand_catalogue".to_string()));
    assert!(names.contains(&"get_facts_in_catalogue".to_string()));
    // Issue #386: the `remember` tool was replaced by the hooks engine and
    // must no longer be exposed to the model.
    assert!(!names.contains(&"remember".to_string()));
}
#[tokio::test]
async fn test_kg_tools_in_openai_export() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    let exported = state.tool_registry.export_openai_tools();
    let names: Vec<String> = exported
        .iter()
        .filter_map(|v| {
            v.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    assert!(names.contains(&"kg_query".to_string()));
    assert!(names.contains(&"kg_related".to_string()));
    assert!(names.contains(&"kg_search".to_string()));
    assert!(names.contains(&"expand_catalogue".to_string()));
    assert!(names.contains(&"get_facts_in_catalogue".to_string()));
    assert!(!names.contains(&"remember".to_string()));
}
#[tokio::test]
async fn test_kb_optimization_status_returns_job() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .uri("/kb/optimization/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::OptimizationStatusResponse =
        serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp.job_id, "knowledge.optimization");
    assert_eq!(resp.priority, "system");
    assert!(resp.schedule.is_some());
}
#[tokio::test]
async fn test_kb_optimization_run_now_triggers_job() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/optimization/run-now")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: mimir_api_types::OptimizationRunNowResponse =
        serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp.status, "succeeded");
}

#[tokio::test]
async fn test_kb_optimization_run_now_cancelled_returns_409() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    // Re-register the optimization job with a non-cooperative slow handler so
    // the run-now cancellation branch wins and the run is recorded as
    // cancelled.
    let slow_job = mimir_core::job_queue::Job::new(
        "knowledge.optimization",
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
                .uri("/kb/optimization/run-now")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });

    let running = poll_until(Duration::from_secs(5), || async {
        state.job_queue.is_running("knowledge.optimization").await
    })
    .await;
    assert!(
        running,
        "knowledge optimization job did not start within 5s"
    );
    assert!(jq.cancel("knowledge.optimization"));

    let response = response_task.await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_kb_optimization_run_now_timed_out_returns_504() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;

    let slow_job = mimir_core::job_queue::Job::new(
        "knowledge.optimization",
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
                .uri("/kb/optimization/run-now")
                .extension(axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                )))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
}
// ------------------------------------------------------------------
// KG CLI route tests
// ------------------------------------------------------------------
#[tokio::test]
async fn test_kb_query_returns_facts() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

    // Seed an entity and a fact
    let entity = state
        .knowledge_graph
        .create_entity(
            "Alice",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("works_at")
        .await
        .unwrap();
    let _fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "works_at".to_string(),
            object_id: None,
            object_literal: Some("Acme".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.95,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/query?entity=Alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!json.get("facts").unwrap().as_array().unwrap().is_empty());
}
#[tokio::test]
async fn test_kb_show_returns_fact_detail() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let entity = state
        .knowledge_graph
        .create_entity(
            "Bob",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("likes")
        .await
        .unwrap();
    let fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "likes".to_string(),
            object_id: None,
            object_literal: Some("Pizza".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.88,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri(format!("/kb/facts/{}", fact.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["fact"]["id"], fact.id);
}

#[tokio::test]
async fn test_kb_show_returns_content_update_audit_entry() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let entity = state
        .knowledge_graph
        .create_entity(
            "Cara",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("likes")
        .await
        .unwrap();
    let fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "likes".to_string(),
            object_id: None,
            object_literal: Some("Pizza".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.88,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let edit = app
        .clone()
        .oneshot(
            authed_request()
                .method("PATCH")
                .uri(format!("/kb/facts/{}", fact.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"object_literal": "Sushi"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);

    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri(format!("/kb/facts/{}", fact.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["audit_log"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["change_type"].as_str() == Some("content_update")),
        "expected a content_update audit entry, got: {}",
        json["audit_log"]
    );
}

#[tokio::test]
async fn test_kb_browse_returns_edges() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let alice = state
        .knowledge_graph
        .create_entity(
            "Alice",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let acme = state
        .knowledge_graph
        .create_entity(
            "Acme",
            mimir_knowledge::models::entity::EntityType::Organization,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("works_at")
        .await
        .unwrap();
    let _fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: alice.id,
            relationship_type: "works_at".to_string(),
            object_id: Some(acme.id),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.92,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/browse?entity=Alice&depth=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!json.get("edges").unwrap().as_array().unwrap().is_empty());
}
#[tokio::test]
async fn test_kb_profile_returns_groups() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let entity = state
        .knowledge_graph
        .create_entity(
            "Charlie",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("enjoys")
        .await
        .unwrap();
    let _fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "enjoys".to_string(),
            object_id: None,
            object_literal: Some("Hiking".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.95,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/profile?entity=Charlie")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["entity_name"], "Charlie");
}
#[tokio::test]
async fn test_kb_audit_returns_entries() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let entity = state
        .knowledge_graph
        .create_entity(
            "Dave",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("lives_in")
        .await
        .unwrap();
    let _fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "lives_in".to_string(),
            object_id: None,
            object_literal: Some("London".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.90,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/audit?entity=Dave")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entries = json["entries"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "expected at least one audit entry (Created)"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["change_type"].as_str() == Some("created")),
        "expected a Created audit entry"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["changed_by"].as_str() == Some("User")),
        "expected the User wire string for the created-by audit entry, got: {}",
        json["entries"]
    );
}

#[tokio::test]
async fn test_kb_audit_and_show_render_same_changed_by_casing() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let entity = state
        .knowledge_graph
        .create_entity(
            "Dana",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("likes")
        .await
        .unwrap();
    let fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "likes".to_string(),
            object_id: None,
            object_literal: Some("Pizza".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.88,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let edit = app
        .clone()
        .oneshot(
            authed_request()
                .method("PATCH")
                .uri(format!("/kb/facts/{}", fact.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"object_literal": "Sushi"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);

    let audit_response = app
        .clone()
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/audit?entity=Dana")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(audit_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let audit_changed_by = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["change_type"].as_str() == Some("content_update"))
        .and_then(|e| e["changed_by"].as_str().map(str::to_string))
        .expect("expected a content_update audit entry from /kb/audit");

    let show_response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri(format!("/kb/facts/{}", fact.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(show_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(show_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let show_changed_by = json["audit_log"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["change_type"].as_str() == Some("content_update"))
        .and_then(|e| e["changed_by"].as_str().map(str::to_string))
        .expect("expected a content_update audit entry from /kb/facts/{id}");

    assert_eq!(audit_changed_by, show_changed_by);
    assert_eq!(audit_changed_by, "User");
}
#[tokio::test]
async fn test_kb_forget_restore_trash_roundtrip() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let entity = state
        .knowledge_graph
        .create_entity(
            "Eve",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    let pred_id = state
        .knowledge_graph
        .ensure_relationship_type("has")
        .await
        .unwrap();
    let fact = mimir_knowledge::queries::fact::insert_fact(
        state.knowledge_graph.pool(),
        &mimir_knowledge::models::fact::NewFact {
            subject_id: entity.id,
            relationship_type: "has".to_string(),
            object_id: None,
            object_literal: Some("Cat".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: mimir_knowledge::models::source::SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        pred_id,
        0.85,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    let app = mimir_server::build_app(state.clone());

    // Forget
    let _forget_resp = app
        .clone()
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/facts/forget")
                .header("Content-Type", "application/json")
                .extension(loopback_connect_info())
                .body(Body::from(
                    serde_json::json!({"fact_id": fact.id}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    // List trash
    let trash_resp = app
        .clone()
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/trash")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trash_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(trash_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let trash_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!trash_json["items"].as_array().unwrap().is_empty());

    // Restore
    let _restore_resp = app
        .clone()
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/trash/restore")
                .header("Content-Type", "application/json")
                .extension(loopback_connect_info())
                .body(Body::from(serde_json::json!({"all": true}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn test_kb_forget_rejects_non_loopback() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/facts/forget")
                .header("Content-Type", "application/json")
                .extension(non_loopback_connect_info())
                .body(Body::from(serde_json::json!({"fact_id": 1}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_kb_trash_empty_rejects_non_loopback() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("DELETE")
                .uri("/kb/trash")
                .extension(non_loopback_connect_info())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_kb_trash_restore_rejects_non_loopback() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
    let app = mimir_server::build_app(state.clone());

    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/trash/restore")
                .header("Content-Type", "application/json")
                .extension(non_loopback_connect_info())
                .body(Body::from(serde_json::json!({"all": true}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
