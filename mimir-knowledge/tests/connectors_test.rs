//! Connector instance-registry facade tests (issue #179 / Phase 3 F2).

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn gmail_input(slug: &str) -> UpsertConnectorInput {
    UpsertConnectorInput {
        connector_type: ConnectorType::Gmail,
        slug: slug.to_string(),
        backend: "imap".to_string(),
        display_name: "Personal Gmail".to_string(),
        config_json: "{}".to_string(),
        status: None,
        auth_state: None,
    }
}

#[tokio::test]
async fn upsert_creates_connector_with_defaults() {
    let (kg, _dir) = init_kg().await;
    let c = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    assert_eq!(c.slug, "personal");
    assert_eq!(c.connector_type_id, ConnectorType::Gmail as i16);
    assert_eq!(c.backend, "imap");
    assert_eq!(c.config_json, "{}");
    assert_eq!(c.status(), Some(ConnectorStatus::Setup));
    assert_eq!(c.auth_state(), Some(ConnectorAuthState::Unauthenticated));
    assert_eq!(c.sync_cursor, None);
    assert_eq!(c.last_sync_at, None);
    assert_eq!(c.last_error, None);
}

#[tokio::test]
async fn get_by_slug_and_id_and_list() {
    let (kg, _dir) = init_kg().await;
    let created = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    let by_slug = kg.get_connector_by_slug("personal").await.unwrap();
    assert_eq!(by_slug.unwrap().id, created.id);

    let by_id = kg.get_connector(created.id).await.unwrap();
    assert_eq!(by_id.unwrap().slug, "personal");

    assert!(kg.get_connector_by_slug("nope").await.unwrap().is_none());
    assert!(kg.get_connector(i32::MAX).await.unwrap().is_none());

    kg.upsert_connector(gmail_input("work")).await.unwrap();
    let all = kg.list_connectors().await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].slug, "personal");
    assert_eq!(all[1].slug, "work");
}

#[tokio::test]
async fn upsert_on_existing_slug_updates_config_preserves_progress() {
    let (kg, _dir) = init_kg().await;
    let original = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    // Advance sync progress via the dedicated mutators.
    kg.update_sync_cursor(original.id, Some("cursor-1"))
        .await
        .unwrap();
    kg.set_connector_status(
        original.id,
        ConnectorStatus::Active,
        Some(Some("boom".to_string())),
    )
    .await
    .unwrap();

    // Re-upsert with a changed config surface and explicit status/auth.
    let updated = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "personal".to_string(),
            backend: "graph".to_string(),
            display_name: "Work Gmail".to_string(),
            config_json: r#"{"folder":"inbox"}"#.to_string(),
            status: Some(ConnectorStatus::Paused),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();

    assert_eq!(updated.id, original.id); // same row
    assert_eq!(updated.backend, "graph");
    assert_eq!(updated.display_name, "Work Gmail");
    assert_eq!(updated.config_json, r#"{"folder":"inbox"}"#);
    assert_eq!(updated.status(), Some(ConnectorStatus::Paused));
    assert_eq!(
        updated.auth_state(),
        Some(ConnectorAuthState::Authenticated)
    );
    // Sync-progress fields preserved across the upsert.
    assert_eq!(updated.sync_cursor, Some("cursor-1".to_string()));
    assert!(updated.last_sync_at.is_some());
    assert_eq!(updated.last_error, Some("boom".to_string()));
}

#[tokio::test]
async fn update_sync_cursor_persists_and_errors_on_missing() {
    let (kg, _dir) = init_kg().await;
    let c = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    let updated = kg.update_sync_cursor(c.id, Some("abc")).await.unwrap();
    assert_eq!(updated.sync_cursor, Some("abc".to_string()));
    assert!(updated.last_sync_at.is_some());

    let cleared = kg.update_sync_cursor(c.id, None).await.unwrap();
    assert_eq!(cleared.sync_cursor, None);

    let err = kg.update_sync_cursor(i32::MAX, None).await;
    assert!(matches!(
        err,
        Err(mimir_knowledge::KnowledgeError::ConnectorNotFound(_))
    ));
}

#[tokio::test]
async fn set_connector_status_set_clear_leave_error() {
    let (kg, _dir) = init_kg().await;
    let c = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    // Set an error message alongside the Error status.
    let errored = kg
        .set_connector_status(
            c.id,
            ConnectorStatus::Error,
            Some(Some("timeout".to_string())),
        )
        .await
        .unwrap();
    assert_eq!(errored.status(), Some(ConnectorStatus::Error));
    assert_eq!(errored.last_error, Some("timeout".to_string()));

    // Leave last_error untouched while changing status.
    let paused = kg
        .set_connector_status(c.id, ConnectorStatus::Paused, None)
        .await
        .unwrap();
    assert_eq!(paused.status(), Some(ConnectorStatus::Paused));
    assert_eq!(paused.last_error, Some("timeout".to_string())); // preserved

    // Explicitly clear last_error.
    let cleared = kg
        .set_connector_status(c.id, ConnectorStatus::Active, Some(None))
        .await
        .unwrap();
    assert_eq!(cleared.status(), Some(ConnectorStatus::Active));
    assert_eq!(cleared.last_error, None);

    // Missing id errors.
    let err = kg
        .set_connector_status(i32::MAX, ConnectorStatus::Active, None)
        .await;
    assert!(matches!(
        err,
        Err(mimir_knowledge::KnowledgeError::ConnectorNotFound(_))
    ));
}

#[tokio::test]
async fn set_auth_state_persists_and_errors_on_missing() {
    let (kg, _dir) = init_kg().await;
    let c = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    let updated = kg
        .set_auth_state(c.id, ConnectorAuthState::Authenticated)
        .await
        .unwrap();
    assert_eq!(
        updated.auth_state(),
        Some(ConnectorAuthState::Authenticated)
    );

    let expired = kg
        .set_auth_state(c.id, ConnectorAuthState::Expired)
        .await
        .unwrap();
    assert_eq!(expired.auth_state(), Some(ConnectorAuthState::Expired));

    let err = kg
        .set_auth_state(i32::MAX, ConnectorAuthState::Authenticated)
        .await;
    assert!(matches!(
        err,
        Err(mimir_knowledge::KnowledgeError::ConnectorNotFound(_))
    ));
}

#[tokio::test]
async fn duplicate_slug_upserts_instead_of_erroring() {
    let (kg, _dir) = init_kg().await;
    let first = kg.upsert_connector(gmail_input("personal")).await.unwrap();
    let second = kg.upsert_connector(gmail_input("personal")).await.unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(kg.list_connectors().await.unwrap().len(), 1);
}
