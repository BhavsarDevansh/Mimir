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
async fn touch_last_sync_preserves_cursor_and_stamps_time() {
    let (kg, _dir) = init_kg().await;
    let c = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    // Set a cursor first.
    let with_cursor = kg.update_sync_cursor(c.id, Some("abc")).await.unwrap();
    assert_eq!(with_cursor.sync_cursor, Some("abc".to_string()));
    let stamp_before = with_cursor.last_sync_at;

    // touch_last_sync must advance last_sync_at without touching the cursor.
    let touched = kg.touch_last_sync(c.id).await.unwrap();
    assert_eq!(touched.sync_cursor, Some("abc".to_string()));
    assert!(touched.last_sync_at >= stamp_before);

    // Missing connector errors.
    let err = kg.touch_last_sync(i32::MAX).await;
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

#[tokio::test]
async fn upsert_rejects_type_mismatch_on_existing_slug() {
    let (kg, _dir) = init_kg().await;
    let original = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    // Reusing the same slug with a different connector type must error rather
    // than silently rewrite the instance's kind (which would leave the previous
    // backend's sync state attached to a different type).
    let mut cal = gmail_input("personal");
    cal.connector_type = ConnectorType::Calendar;
    let err = kg.upsert_connector(cal).await;
    assert!(matches!(
        err,
        Err(mimir_knowledge::KnowledgeError::ConnectorTypeMismatch(ref s))
        if s == "personal"
    ));

    // The original row is untouched: still Gmail, still Setup/Unauthenticated.
    let stored = kg.get_connector_by_slug("personal").await.unwrap().unwrap();
    assert_eq!(stored.id, original.id);
    assert_eq!(stored.connector_type_id, ConnectorType::Gmail as i16);
    assert_eq!(stored.status(), Some(ConnectorStatus::Setup));

    // A same-type re-upsert still updates the mutable surface.
    let same_type = UpsertConnectorInput {
        connector_type: ConnectorType::Gmail,
        slug: "personal".to_string(),
        backend: "graph".to_string(),
        display_name: "Renamed".to_string(),
        config_json: "{}".to_string(),
        status: Some(ConnectorStatus::Active),
        auth_state: None,
    };
    let updated = kg.upsert_connector(same_type).await.unwrap();
    assert_eq!(updated.id, original.id);
    assert_eq!(updated.connector_type_id, ConnectorType::Gmail as i16);
    assert_eq!(updated.backend, "graph");
    assert_eq!(updated.status(), Some(ConnectorStatus::Active));
}

#[tokio::test]
async fn count_sources_for_connector_returns_zero_for_unknown() {
    let (kg, _dir) = init_kg().await;
    assert_eq!(kg.count_sources_for_connector(i32::MAX).await.unwrap(), 0);
}

#[tokio::test]
async fn delete_connector_removes_row_and_errors_on_missing() {
    let (kg, _dir) = init_kg().await;
    let c = kg.upsert_connector(gmail_input("personal")).await.unwrap();

    kg.delete_connector(c.id).await.unwrap();
    assert!(kg.get_connector(c.id).await.unwrap().is_none());
    assert!(
        kg.get_connector_by_slug("personal")
            .await
            .unwrap()
            .is_none()
    );

    let err = kg.delete_connector(c.id).await;
    assert!(matches!(
        err,
        Err(mimir_knowledge::KnowledgeError::ConnectorNotFound(_))
    ));
}

#[tokio::test]
async fn count_sources_by_connector_groups_in_one_query() {
    let (kg, _dir) = init_kg().await;
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

    let alice = kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap()
        .id;
    let london = kg
        .create_entity("London", EntityType::Place, &[])
        .await
        .unwrap()
        .id;
    let g = kg.upsert_connector(gmail_input("g")).await.unwrap();
    let c = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: mimir_knowledge::models::enums::ConnectorType::Calendar,
            slug: "c".to_string(),
            backend: "caldav".to_string(),
            display_name: "C".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    let mk = |instance_id: i32, raw: &str| NewFact {
        subject_id: alice,
        relationship_type: "is_in".to_string(),
        object_id: Some(london),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_id),
        connector_type: None,
        raw_reference: Some(raw.to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    kg.insert_fact(mk(g.id, "g-1")).await.unwrap();
    kg.insert_fact(mk(g.id, "g-2")).await.unwrap();
    kg.insert_fact(mk(c.id, "c-1")).await.unwrap();

    let counts = kg.count_sources_by_connector().await.unwrap();
    assert_eq!(counts.get(&g.id).copied(), Some(2));
    assert_eq!(counts.get(&c.id).copied(), Some(1));
}

// ---------------------------------------------------------------------------
// Atomic create-only insert (#202 review): unique-slug enforcement at the DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_connector_inserts_a_new_instance() {
    let (kg, _dir) = init_kg().await;
    let c = kg.create_connector(gmail_input("personal")).await.unwrap();
    assert_eq!(c.slug, "personal");
    assert_eq!(c.connector_type_id, ConnectorType::Gmail as i16);
    assert_eq!(c.status(), Some(ConnectorStatus::Setup));
    assert_eq!(c.auth_state(), Some(ConnectorAuthState::Unauthenticated));

    // A duplicate slug is rejected atomically with the dedicated error.
    let err = kg.create_connector(gmail_input("personal")).await;
    match err {
        Err(mimir_knowledge::KnowledgeError::ConnectorSlugConflict(slug)) => {
            assert_eq!(slug, "personal");
        }
        other => panic!("expected ConnectorSlugConflict, got {other:?}"),
    }
    // The original row is untouched.
    assert_eq!(kg.list_connectors().await.unwrap().len(), 1);
}

/// Two concurrent `create_connector` writes for the same slug must not both
/// succeed: the database-level unique constraint lets exactly one win and
/// surfaces the other as `ConnectorSlugConflict` (#202 review).
#[tokio::test]
async fn create_connector_concurrent_same_slug_yields_one_winner() {
    use std::sync::Arc;
    let (kg, _dir) = init_kg().await;
    let kg = Arc::new(kg);

    // A barrier so both tasks race the insert at the same instant.
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let kg_a = kg.clone();
    let kg_b = kg.clone();
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();

    let a = tokio::spawn(async move {
        barrier_a.wait().await;
        kg_a.create_connector(gmail_input("race")).await
    });
    let b = tokio::spawn(async move {
        barrier_b.wait().await;
        kg_b.create_connector(gmail_input("race")).await
    });

    let ra = a.await.unwrap();
    let rb = b.await.unwrap();

    // Exactly one succeeds; the other gets a slug conflict.
    let wins = [&ra, &rb].iter().filter(|r| r.is_ok()).count();
    let conflicts = [&ra, &rb]
        .iter()
        .filter(|r| {
            matches!(
                r,
                Err(mimir_knowledge::KnowledgeError::ConnectorSlugConflict(_))
            )
        })
        .count();
    assert_eq!(wins, 1, "exactly one concurrent create must succeed");
    assert_eq!(
        conflicts, 1,
        "the losing concurrent create must be a slug conflict"
    );

    // Only one row exists for the slug.
    let rows = kg.list_connectors().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].slug, "race");
}

// -- forget cascade (Phase 3 A2 / #203) --

use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries::source::AddSourceRequest;

/// Insert a fact sourced from a connector instance (mimicking ingestion).
async fn connector_sourced_fact(kg: &KnowledgeGraph, instance_id: i32, value: &str) -> i32 {
    let entity = kg
        .create_entity(&format!("Source-{value}"), EntityType::Concept, &[])
        .await
        .unwrap();
    let mut nf = NewFact::new(entity.id, "has_name");
    nf.object_literal = Some(value.to_string());
    let fact = kg.insert_fact(nf).await.unwrap();
    kg.add_source_to_fact(AddSourceRequest {
        fact_id: fact.id,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_id),
        connector_type: Some(ConnectorType::Gmail),
        raw_reference: Some(format!("raw-{value}")),
        extraction_method: None,
        changed_by: ChangedBy::System,
    })
    .await
    .unwrap();
    fact.id
}

#[tokio::test]
async fn forget_connector_facts_trashes_sourced_facts() {
    let (kg, _dir) = init_kg().await;
    let connector = kg.create_connector(gmail_input("forget-me")).await.unwrap();
    let other = kg.create_connector(gmail_input("keep-me")).await.unwrap();

    let f1 = connector_sourced_fact(&kg, connector.id, "alpha").await;
    let f2 = connector_sourced_fact(&kg, connector.id, "beta").await;
    let _f3 = connector_sourced_fact(&kg, other.id, "gamma").await;

    // Two facts are sourced from `connector`, one from `other`.
    assert_eq!(
        kg.count_sources_for_connector(connector.id).await.unwrap(),
        2
    );

    let result = kg
        .forget_connector_facts(connector.id, ChangedBy::User)
        .await
        .unwrap();
    assert_eq!(result.forgotten_count, 2);

    // The connector's facts are gone; the other connector's fact survives.
    assert_eq!(
        kg.count_sources_for_connector(connector.id).await.unwrap(),
        0
    );
    assert_eq!(kg.count_sources_for_connector(other.id).await.unwrap(), 1);
    // The facts themselves are no longer active (cascade-deleted sources).
    assert!(kg.get_fact(f1).await.unwrap().is_none());
    assert!(kg.get_fact(f2).await.unwrap().is_none());
    // The two trashed facts are recoverable from trash.
    let trash = kg.list_trash(100, 0).await.unwrap();
    assert_eq!(trash.len(), 2);
}

#[tokio::test]
async fn forget_connector_facts_no_sources_is_zero() {
    let (kg, _dir) = init_kg().await;
    let connector = kg.create_connector(gmail_input("empty")).await.unwrap();
    let result = kg
        .forget_connector_facts(connector.id, ChangedBy::User)
        .await
        .unwrap();
    assert_eq!(result.forgotten_count, 0);
}

/// A fact sourced from *both* the connector and an independent source (a chat
/// turn) is trashed wholesale by the connector cascade: the connector source
/// is the trigger, and the fact is recoverable from trash.
#[tokio::test]
async fn forget_connector_facts_trashes_multi_source_facts() {
    let (kg, _dir) = init_kg().await;
    let connector = kg.create_connector(gmail_input("multi")).await.unwrap();

    let fact_id = connector_sourced_fact(&kg, connector.id, "shared").await;
    // A second, connector-independent source for the same fact.
    kg.add_source_to_fact(AddSourceRequest {
        fact_id,
        source_type: SourceType::Interaction,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: Some("chat-1".to_string()),
        extraction_method: None,
        changed_by: ChangedBy::User,
    })
    .await
    .unwrap();

    let result = kg
        .forget_connector_facts(connector.id, ChangedBy::User)
        .await
        .unwrap();
    assert_eq!(result.forgotten_count, 1);
    assert!(kg.get_fact(fact_id).await.unwrap().is_none());
    assert_eq!(kg.list_trash(100, 0).await.unwrap().len(), 1);
}
