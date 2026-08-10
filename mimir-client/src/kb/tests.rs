//! Wiremock-backed tests for the KB client commands.

use mimir_api_types::*;

use crate::MimirClient;
use crate::error::ClientError;

#[allow(unused_imports)]
use mimir_api_types::{
    AuditRow, BrowseEdge, ChatMessage, ChatRequest, FactRow, OptimizationStatusResponse,
    PendingFactRow, ProfileGroup, TrashRow, Usage,
};
#[allow(unused_imports)]
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

#[tokio::test]
async fn test_kb_categories_parsing() {
    let server = MockServer::start().await;
    let payload = vec![CategoryResponse {
        id: 1,
        name: "People".to_string(),
        description: None,
        parent_id: None,
        memory_weight: Some(1.0),
    }];
    Mock::given(method("GET"))
        .and(path("/kb/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let cats = client.kb_categories(None).await.unwrap();
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].id, 1);
}

#[tokio::test]
async fn test_kb_category_show_parsing() {
    let server = MockServer::start().await;
    let payload = CategoryDetailResponse {
        id: 1,
        name: "People".to_string(),
        description: None,
        parent_id: None,
        memory_weight: Some(1.0),
        fact_count: 5,
        children: vec![],
    };
    Mock::given(method("GET"))
        .and(path("/kb/categories/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let cat = client.kb_category_show(1).await.unwrap();
    assert_eq!(cat.id, 1);
    assert_eq!(cat.fact_count, 5);
}

#[tokio::test]
async fn test_kb_category_create_and_delete() {
    let server = MockServer::start().await;
    let payload = CategoryResponse {
        id: 42,
        name: "Places".to_string(),
        description: None,
        parent_id: None,
        memory_weight: Some(1.0),
    };
    Mock::given(method("POST"))
        .and(path("/kb/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/kb/categories/42"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let cat = client
        .kb_category_create(42, "Places".to_string(), None, None, None)
        .await
        .unwrap();
    assert_eq!(cat.id, 42);
    client.kb_category_delete(42).await.unwrap();
}

// ---- integration tests for previously-uncovered client methods ---------
async fn sample_fact_row() -> FactRow {
    FactRow {
        id: 7,
        subject: "Alice".to_string(),
        predicate: "lives_in".to_string(),
        object: Some("London".to_string()),
        confidence: 0.9,
        status: "active".to_string(),
        valid_from: Some("2020-01-01T00:00:00Z".to_string()),
        valid_until: None,
        inferred: false,
    }
}

#[tokio::test]
async fn test_kb_query_with_filters() {
    let server = MockServer::start().await;
    let payload = FactQueryResponse {
        total: 1,
        offset: 0,
        limit: 10,
        facts: vec![sample_fact_row().await],
    };
    Mock::given(method("GET"))
        .and(path("/kb/query"))
        .and(query_param("entity", "Alice"))
        .and(query_param("predicate", "lives_in"))
        .and(query_param("min_confidence", "0.5"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let req = FactQueryParams {
        entity: "Alice".to_string(),
        predicate: Some("lives_in".to_string()),
        min_confidence: Some(0.5),
        offset: Some(0),
        limit: Some(10),
    };
    let result = client.kb_query(req).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.facts.len(), 1);
    assert_eq!(result.facts[0].subject, "Alice");
}

#[tokio::test]
async fn test_kb_show() {
    let server = MockServer::start().await;
    let payload = FactDetailResponse {
        fact: sample_fact_row().await,
        sources: vec![],
        dependencies: vec![],
        audit_log: vec![],
    };
    Mock::given(method("GET"))
        .and(path("/kb/facts/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let result = client.kb_show(7).await.unwrap();
    assert_eq!(result.fact.id, 7);
    assert!(result.sources.is_empty());
}

#[tokio::test]
async fn test_kb_edit() {
    let server = MockServer::start().await;
    let payload = FactEditResponse {
        fact: sample_fact_row().await,
    };
    Mock::given(method("PATCH"))
        .and(path("/kb/facts/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let req = FactEditRequest {
        confidence: Some(0.8),
        valid_from: None,
        valid_until: None,
        object_literal: Some("London".to_string()),
        status: None,
    };
    let result = client.kb_edit(7, req).await.unwrap();
    assert_eq!(result.fact.id, 7);
}

#[tokio::test]
async fn test_kb_browse() {
    let server = MockServer::start().await;
    let payload = BrowseResponse {
        total_edges: 1,
        offset: 0,
        limit: 10,
        edges: vec![BrowseEdge {
            depth: 1,
            subject: "Alice".to_string(),
            predicate: "lives_in".to_string(),
            object: "London".to_string(),
            confidence: 0.9,
        }],
    };
    Mock::given(method("GET"))
        .and(path("/kb/browse"))
        .and(query_param("entity", "Alice"))
        .and(query_param("depth", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let req = BrowseRequest {
        entity: "Alice".to_string(),
        depth: 2,
        offset: Some(0),
        limit: Some(10),
    };
    let result = client.kb_browse(req).await.unwrap();
    assert_eq!(result.total_edges, 1);
    assert_eq!(result.edges[0].object, "London");
}

#[tokio::test]
async fn test_kb_profile() {
    let server = MockServer::start().await;
    let payload = ProfileResponse {
        entity_name: "Alice".to_string(),
        groups: vec![ProfileGroup {
            category: "personal".to_string(),
            facts: vec![sample_fact_row().await],
        }],
    };
    Mock::given(method("GET"))
        .and(path("/kb/profile"))
        .and(query_param("entity", "Alice"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let result = client
        .kb_profile(ProfileRequest {
            entity: Some("Alice".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(result.entity_name, "Alice");
    assert_eq!(result.groups.len(), 1);
}

#[tokio::test]
async fn test_kb_audit() {
    let server = MockServer::start().await;
    let payload = AuditQueryResponse {
        total: 1,
        offset: 0,
        limit: 10,
        entries: vec![AuditRow {
            audit_id: 1,
            fact_id: 7,
            change_type: "status_change".to_string(),
            entity_name: Some("Alice".to_string()),
            predicate_name: Some("lives_in".to_string()),
            old_value: None,
            new_value: Some("London".to_string()),
            changed_at: "2020-01-01T00:00:00Z".to_string(),
            changed_by: None,
            reason: None,
        }],
    };
    Mock::given(method("GET"))
        .and(path("/kb/audit"))
        .and(query_param("entity", "Alice"))
        .and(query_param("change_type", "status_change"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let req = AuditQueryRequest {
        entity: Some("Alice".to_string()),
        predicate: None,
        from: None,
        to: None,
        change_type: Some("status_change".to_string()),
        offset: Some(0),
        limit: Some(10),
    };
    let result = client.kb_audit(req).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.entries[0].fact_id, 7);
}

#[tokio::test]
async fn test_kb_forget() {
    let server = MockServer::start().await;
    let payload = ForgetResponse {
        forgotten_count: 3,
        backup_path: Some("/tmp/backup.json".to_string()),
    };
    Mock::given(method("POST"))
        .and(path("/kb/facts/forget"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let req = ForgetRequest {
        fact_id: Some(7),
        predicate: None,
        subject: None,
        entity: None,
        source: None,
        from: None,
        to: None,
        all: false,
        yes: false,
        confirm_sensitive: false,
        confirmation_phrase: None,
        archive: false,
    };
    let result = client.kb_forget(req).await.unwrap();
    assert_eq!(result.forgotten_count, 3);
    assert!(result.backup_path.is_some());
}

#[tokio::test]
async fn test_kb_restore() {
    let server = MockServer::start().await;
    let payload = RestoreResponse { restored_count: 2 };
    Mock::given(method("POST"))
        .and(path("/kb/trash/restore"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let result = client
        .kb_restore(RestoreRequest {
            trash_id: Some(1),
            all: false,
        })
        .await
        .unwrap();
    assert_eq!(result.restored_count, 2);
}

#[tokio::test]
async fn test_kb_trash_list() {
    let server = MockServer::start().await;
    let payload = TrashListResponse {
        total: 1,
        offset: 0,
        limit: 10,
        items: vec![TrashRow {
            trash_id: 1,
            subject: Some("Alice".to_string()),
            predicate: None,
            object: None,
            deleted_at: "2020-01-01T00:00:00Z".to_string(),
            expires_at: "2021-01-01T00:00:00Z".to_string(),
        }],
    };
    Mock::given(method("GET"))
        .and(path("/kb/trash"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let result = client.kb_trash(0, 10).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items.len(), 1);
}

#[tokio::test]
async fn test_kb_trash_empty_success() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/kb/trash"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    client.kb_trash_empty().await.unwrap();
}

#[tokio::test]
async fn test_kb_trash_empty_error() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/kb/trash"))
        .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let err = client.kb_trash_empty().await.unwrap_err();
    assert!(matches!(err, ClientError::Server { status: 500, message } if message == "oops"));
}

#[tokio::test]
async fn test_kb_pending() {
    let server = MockServer::start().await;
    let payload = PendingListResponse {
        total: 1,
        facts: vec![PendingFactRow {
            fact_id: 7,
            subject: "Alice".to_string(),
            predicate: "ssn".to_string(),
            object: None,
            created_at: "2020-01-01T00:00:00Z".to_string(),
        }],
    };
    Mock::given(method("GET"))
        .and(path("/kb/pending"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let result = client.kb_pending().await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.facts[0].fact_id, 7);
}

#[tokio::test]
async fn test_kb_confirm() {
    let server = MockServer::start().await;
    let payload = ConfirmFactResponse {
        fact: sample_fact_row().await,
    };
    Mock::given(method("POST"))
        .and(path("/kb/facts/7/confirm"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let result = client.kb_confirm(7).await.unwrap();
    assert_eq!(result.fact.id, 7);
}

#[tokio::test]
async fn test_kb_reject_with_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/kb/facts/7/reject"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    client.kb_reject(7, Some("entered in error")).await.unwrap();
}

#[tokio::test]
async fn test_kb_reject_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/kb/facts/7/reject"))
        .respond_with(ResponseTemplate::new(404).set_body_string("no such fact"))
        .mount(&server)
        .await;

    let client = MimirClient::new(server.uri());
    let err = client.kb_reject(7, None).await.unwrap_err();
    assert!(
        matches!(err, ClientError::Server { status: 404, message } if message == "no such fact")
    );
}
