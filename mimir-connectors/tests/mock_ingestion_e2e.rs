//! T1 integration test (Phase 3 F13 / #190): the `MockConnector` end-to-end
//! vehicle.
//!
//! Drives the **real** `ConnectorSupervisor` + `KnowledgeGraph` against a
//! `MockConnector` configured to emit canned facts, proving the full
//! sync → extract → `normalize_and_insert` → query path works under both
//! `Polling` and `Push` modes without any real service. This is the T1 harness
//! the Phase 3 plan references; server/HTTP-level E2E stays in the separate T1
//! issue.

#![cfg(feature = "test-mock-connector")]
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use mimir_connectors::{
    ConnectorRegistry, ConnectorSupervisor, MockConnectorFactory, MockFactConfig, SupervisorConfig,
    SyncOptions,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{
    ConnectorAuthState, ConnectorStatus, ConnectorType, RecurrenceType,
};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn fast_config() -> SupervisorConfig {
    SupervisorConfig {
        max_failures: 5,
        base_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

/// A canned fact with a literal object (no entity resolution on the object).
fn literal_fact(subject: &str, rel: &str, object: &str, raw: &str) -> MockFactConfig {
    MockFactConfig {
        subject: subject.to_string(),
        subject_type: EntityType::Person,
        relationship_type: rel.to_string(),
        object: object.to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: Some(raw.to_string()),
        location: None,
    }
}

/// Register the mock factory under `(Gmail, "mock")` so the supervisor can
/// instantiate the configured connector row.
fn mock_registry() -> Arc<ConnectorRegistry> {
    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Email, "mock", MockConnectorFactory)
        .unwrap();
    Arc::new(registry)
}

fn upsert_mock(slug: &str, config: serde_json::Value) -> UpsertConnectorInput {
    UpsertConnectorInput {
        connector_type: ConnectorType::Email,
        slug: slug.to_string(),
        backend: "mock".to_string(),
        display_name: slug.to_string(),
        config_json: serde_json::to_string(&config).unwrap(),
        status: Some(ConnectorStatus::Active),
        auth_state: Some(ConnectorAuthState::Authenticated),
    }
}

async fn wait_for<F, Fut>(predicate: F, timeout: Duration)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("wait_for timed out after {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Find the entity id for `name` via the public alias/FTS5 search, requiring
/// an exact-name hit. Returns `None` when no such entity exists yet, so a poll
/// loop can distinguish "not yet" from "never".
async fn entity_id(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
    let results = kg.search_entities(name, 10).await.unwrap();
    results
        .into_iter()
        .find(|r| r.entity.name == name)
        .map(|r| r.entity.id)
}

/// Assert connector provenance on a fact: one source of type `Connector`,
/// tied to the instance, with the expected raw reference and method.
async fn assert_connector_source(
    kg: &KnowledgeGraph,
    fact_id: i32,
    instance_id: i32,
    raw_reference: &str,
) {
    let sources = kg.get_sources_for_fact(fact_id).await.unwrap();
    assert!(
        sources.iter().any(|s| {
            s.source_type_id == SourceType::Connector as i16
                && s.connector_instance_id == Some(instance_id)
                && s.connector_type_id == Some(ConnectorType::Email as i16)
                && s.raw_reference.as_deref() == Some(raw_reference)
                && s.extraction_method_id == Some(ExtractionMethod::StructuredParse as i16)
        }),
        "no connector source with instance {instance_id} and raw_reference {raw_reference:?} on fact {fact_id}; sources: {sources:?}"
    );
}

// ---------------------------------------------------------------------------
// Polling: canned facts flow through the supervisor into the KB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn polling_mock_syncs_canned_facts_into_kb() {
    let (kg, _dir) = init_kg().await;
    let config = json!({
        "__slug": "poll",
        "mode": "polling",
        "interval_ms": 100,
        "jitter_ms": 0,
        "cursor": "v1",
        "facts": [
            literal_fact("Alice Mock", "works_at", "Acme", "m-1"),
            literal_fact("Bob Mock", "lives_in", "London", "m-2"),
        ],
    });
    let row = kg
        .upsert_connector(upsert_mock("poll", config))
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(mock_registry(), kg.clone(), fast_config(), rx);
    assert_eq!(
        supervisor.restore().await.unwrap(),
        1,
        "one connector spawned"
    );

    // Wait for Alice's fact to land and the cursor to persist.
    wait_for(
        || async {
            let row_ok = kg
                .get_connector(row.id)
                .await
                .unwrap()
                .map(|c| c.sync_cursor.as_deref() == Some("v1"))
                .unwrap_or(false);
            let Some(alice) = entity_id(&kg, "Alice Mock").await else {
                return false;
            };
            row_ok
                && !kg
                    .get_facts_by_subject(alice, 100)
                    .await
                    .unwrap()
                    .is_empty()
        },
        Duration::from_secs(5),
    )
    .await;

    // Alice's "works_at Acme" fact is present.
    let alice = entity_id(&kg, "Alice Mock")
        .await
        .expect("Alice Mock entity");
    let facts = kg.get_facts_by_subject(alice, 100).await.unwrap();
    let works_at = facts
        .iter()
        .find(|f| f.object_literal.as_deref() == Some("Acme"))
        .expect("works_at Acme fact not found");
    assert!(
        (works_at.confidence - 0.85).abs() < 0.001,
        "expected the Gmail reliability score (0.85), got {}",
        works_at.confidence
    );
    assert_connector_source(&kg, works_at.id, row.id, "m-1").await;

    // Bob's "lives_in London" fact is present too.
    let bob = entity_id(&kg, "Bob Mock").await.expect("Bob Mock entity");
    let bob_facts = kg.get_facts_by_subject(bob, 100).await.unwrap();
    assert!(
        bob_facts
            .iter()
            .any(|f| f.object_literal.as_deref() == Some("London"))
    );

    // The cursor persisted.
    let after = kg.get_connector(row.id).await.unwrap().unwrap();
    assert_eq!(after.sync_cursor.as_deref(), Some("v1"));
    assert_eq!(after.status(), Some(ConnectorStatus::Active));

    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Tombstones: server-side deletions flow through the supervisor into the KB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_tombstones_trash_kb_facts_and_are_idempotent() {
    let (kg, _dir) = init_kg().await;
    // `batch_size: 1` delivers the fact on the first successful sync and the
    // deletion is re-staged every sync, so the flow is: cycle 1 inserts the
    // fact (the tombstone trashes nothing yet), cycle 2 trashes it, and every
    // later cycle re-reports the tombstone with a no-op (idempotent).
    let config = json!({
        "__slug": "tomb",
        "mode": "polling",
        "interval_ms": 100,
        "jitter_ms": 0,
        "batch_size": 1,
        "facts": [literal_fact("Alice Tomb", "works_at", "Acme", "m-del-1")],
        "deletions": ["m-del-1"],
    });
    let row = kg
        .upsert_connector(upsert_mock("tomb", config))
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(mock_registry(), kg.clone(), fast_config(), rx);
    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // The fact lands first.
    wait_for(
        || async {
            let Some(alice) = entity_id(&kg, "Alice Tomb").await else {
                return false;
            };
            !kg.get_facts_by_subject(alice, 100)
                .await
                .unwrap()
                .is_empty()
        },
        Duration::from_secs(5),
    )
    .await;

    // The tombstone cycle trashes it; re-reported tombstones stay no-ops.
    wait_for(
        || async {
            let Some(alice) = entity_id(&kg, "Alice Tomb").await else {
                return true;
            };
            let no_facts = kg
                .get_facts_by_subject(alice, 100)
                .await
                .unwrap()
                .is_empty();
            let in_trash = !kg.list_trash(100, 0).await.unwrap().is_empty();
            no_facts && in_trash
        },
        Duration::from_secs(5),
    )
    .await;

    // Give re-staged tombstones a cycle to prove they do not error and the
    // facts do not resurrect.
    tokio::time::sleep(Duration::from_millis(350)).await;
    let alice = entity_id(&kg, "Alice Tomb").await.expect("entity persists");
    assert!(
        kg.get_facts_by_subject(alice, 100)
            .await
            .unwrap()
            .is_empty(),
        "trashed facts must not resurrect on re-reported tombstones"
    );
    assert_eq!(
        kg.get_connector(row.id).await.unwrap().unwrap().status(),
        Some(ConnectorStatus::Active)
    );

    supervisor.shutdown().await;
}

// ---------------------------------------------------------------------------
// Push: the self-paced loop also lands facts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_mock_syncs_canned_facts_into_kb() {
    let (kg, _dir) = init_kg().await;
    let config = json!({
        "__slug": "push",
        "mode": "push",
        "interval_ms": 40,
        "cursor": "pc",
        "facts": [literal_fact("Cara Push", "knows", "Dan", "p-1")],
    });
    let row = kg
        .upsert_connector(upsert_mock("push", config))
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(mock_registry(), kg.clone(), fast_config(), rx);
    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // The push loop self-paces; the first cycle runs immediately and emits.
    wait_for(
        || async {
            let Some(cara) = entity_id(&kg, "Cara Push").await else {
                return false;
            };
            !kg.get_facts_by_subject(cara, 100).await.unwrap().is_empty()
        },
        Duration::from_secs(5),
    )
    .await;

    let cara = entity_id(&kg, "Cara Push").await.expect("Cara Push entity");
    let facts = kg.get_facts_by_subject(cara, 100).await.unwrap();
    let knows = facts
        .iter()
        .find(|f| f.object_literal.as_deref() == Some("Dan"))
        .expect("knows Dan fact not found");
    assert_connector_source(&kg, knows.id, row.id, "p-1").await;

    // Manual triggers are unsupported for push connectors (F9 contract).
    let err = supervisor
        .trigger_sync(row.id, SyncOptions::default())
        .await
        .err()
        .unwrap();
    assert!(matches!(
        err,
        mimir_connectors::TriggerError::PushUnsupported { .. }
    ));

    supervisor.shutdown().await;
}
