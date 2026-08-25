//! `GET /kb/export` + `POST /kb/import` route tests (issue #62).

mod common;
use common::*;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use mimir_api_types::{ExportResponse, ImportResponse};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::{NormalizedFact, Provenance, normalize_and_insert};
use tower::ServiceExt;

fn loopback() -> axum::extract::ConnectInfo<mimir_server::LocalPeer> {
    axum::extract::ConnectInfo(mimir_server::LocalPeer::Tcp(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        0,
    ))))
}

/// Seed one person with a couple of facts through the shared pipeline so the
/// export has an entity document, a wiki-link, and an event overlay.
async fn seed_graph(state: &Arc<AppState>) {
    let facts = vec![
        NormalizedFact {
            source_type: SourceType::UserEdit,
            subject: "Devansh".to_string(),
            subject_type: EntityType::Person,
            relationship_type: "married_to".to_string(),
            object: "Alice".to_string(),
            object_is_entity: true,
            object_type: Some(EntityType::Person),
            valid_from: Some("2022-01-01T00:00:00Z".parse().unwrap()),
            valid_until: None,
            is_sensitive: false,
            is_correction: false,
            correction_scope: None,
            category_ids: Vec::new(),
            recurrence: RecurrenceType::None,
            requires_user_action: false,
            raw_reference: None,
            extraction_method: Some(mimir_knowledge::models::source::ExtractionMethod::UserInput),
            event_type: None,
            location: None,
            confidence: None,
        },
        NormalizedFact {
            source_type: SourceType::UserEdit,
            subject: "Devansh".to_string(),
            subject_type: EntityType::Person,
            relationship_type: "birthday".to_string(),
            object: "1995-08-20".to_string(),
            object_is_entity: false,
            object_type: None,
            valid_from: Some("1995-08-20T00:00:00Z".parse().unwrap()),
            valid_until: None,
            is_sensitive: false,
            is_correction: false,
            correction_scope: None,
            category_ids: Vec::new(),
            recurrence: RecurrenceType::Yearly,
            requires_user_action: false,
            raw_reference: None,
            extraction_method: Some(mimir_knowledge::models::source::ExtractionMethod::UserInput),
            event_type: Some(EventType::Birthday),
            location: None,
            confidence: None,
        },
    ];
    let outcome = normalize_and_insert(
        &state.knowledge_graph,
        facts,
        Provenance::chat(mimir_knowledge::models::source::ExtractionMethod::UserInput),
    )
    .await
    .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

#[tokio::test]
async fn test_kb_export_returns_rendered_bundle() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    seed_graph(&state).await;

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("GET")
                .uri("/kb/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let export: ExportResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(export.entity_count, 2, "Devansh + Alice");
    assert_eq!(export.fact_count, 2);
    assert_eq!(export.event_count, 1, "birthday event overlay");

    let devansh = export
        .files
        .iter()
        .find(|f| f.relative_path == "Devansh.md")
        .expect("Devansh.md in export");
    assert!(devansh.content.contains("# Devansh\n"));
    assert!(devansh.content.contains("## Dates"));
    assert!(devansh.content.contains("[[Alice]]"));
}

#[tokio::test]
async fn test_kb_import_dry_run_reports_without_writing() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let vault = tempfile::tempdir().unwrap();
    std::fs::write(
        vault.path().join("Devansh.md"),
        "---\ntype: Person\n---\n\n# Devansh\n\n## Relationships\n- married_to → Alice (since 2022-01-01)\n",
    )
    .unwrap();

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/import")
                .extension(loopback())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&mimir_api_types::ImportRequest {
                        path: vault.path().to_string_lossy().into_owned(),
                        dry_run: true,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: ImportResponse = serde_json::from_slice(&body).unwrap();
    assert!(resp.dry_run);
    assert_eq!(resp.entities_new, 2, "Devansh + Alice planned");
    assert_eq!(resp.facts_new, 1);
    assert!(resp.errors.is_empty(), "{:?}", resp.errors);

    // Nothing was written.
    let entities = state
        .knowledge_graph
        .search_entities("Devansh", 10)
        .await
        .unwrap();
    assert!(entities.is_empty(), "dry-run must not write");
}

#[tokio::test]
async fn test_kb_import_applies_and_skips_existing() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let vault = tempfile::tempdir().unwrap();
    let content = "---\ntype: Person\n---\n\n# Devansh\n\n## Relationships\n- married_to → [[Alice]] (since 2022-01-01)\n";
    std::fs::write(vault.path().join("Devansh.md"), content).unwrap();

    let import = || {
        let state = Arc::clone(&state);
        let vault = vault.path().to_path_buf();
        async move {
            let app = mimir_server::build_app(state.clone());
            app.oneshot(
                authed_request()
                    .method("POST")
                    .uri("/kb/import")
                    .extension(loopback())
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&mimir_api_types::ImportRequest {
                            path: vault.to_string_lossy().into_owned(),
                            dry_run: false,
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let response = import().await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: ImportResponse = serde_json::from_slice(&body).unwrap();
    assert!(!resp.dry_run);
    assert_eq!(resp.entities_new, 2);
    assert_eq!(resp.facts_new, 1);
    assert_eq!(resp.facts_existing, 0);

    // Re-import: the exact triple already exists and is skipped.
    let response = import().await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: ImportResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp.entities_new, 0);
    assert_eq!(resp.facts_new, 0);
    assert_eq!(resp.facts_existing, 1);
}

#[tokio::test]
async fn test_kb_import_rejects_missing_directory() {
    let mock = Arc::new(MockLlmClient::builder().build());
    let (state, _temp) = test_state(mock).await;
    let missing = PathBuf::from("/nonexistent/obsidian-vault-62");

    let app = mimir_server::build_app(state.clone());
    let response = app
        .oneshot(
            authed_request()
                .method("POST")
                .uri("/kb/import")
                .extension(loopback())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&mimir_api_types::ImportRequest {
                        path: missing.to_string_lossy().into_owned(),
                        dry_run: true,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
