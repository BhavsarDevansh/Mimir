//! F13 behavioural tests (issue #190): the configurable, always-compiled
//! `MockConnector` test harness.
//!
//! These reference the configurable mock surface (`MockConnector::from_config`,
//! `MockFactConfig`, `MockSyncRecorder`) *before* it exists, so they fail to
//! compile until F13 lands — the TDD anchor.
//!
//! Design (locked with the user):
//! - The mock is **always compiled** (no feature flag) and is the framework's
//!   test harness + the T1 sync→extract→insert vehicle.
//! - It is fully config-driven: behaviour (mode, cadence, canned facts, health,
//!   failure/panic injection, cursor) is read from `config_json`. Instance
//!   identity (`__slug` / `__ctype` / `__instance_id`) is injected by the
//!   supervisor at restore time and read here.
//! - Both `Polling` and `Push` modes are supported. Push self-paces via an
//!   internal `tokio::time::sleep` inside `sync()` (the supervisor aborts the
//!   task on shutdown for cancellation); F9 manual triggers are rejected for
//!   push connectors, so push needs no trigger path.
//! - `MockConnector::default()` preserves the legacy no-op identity
//!   (`id "mock"`, `name "Mock Connector"`, type `Gmail`, `Polling`, health
//!   `Online`, empty `extract`) so existing trait tests keep passing.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use mimir_connectors::{
    Connector, ConnectorError, ConnectorMode, ConnectorRegistry, HealthStatus, MockConnector,
    MockConnectorFactory, MockFactConfig, MockSyncRecorder, SyncOptions,
};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::NormalizedFact;

// ---------------------------------------------------------------------------
// Default preserves the legacy no-op identity
// ---------------------------------------------------------------------------

#[test]
fn default_preserves_legacy_identity() {
    let mock = MockConnector::default();
    assert_eq!(mock.id(), "mock");
    assert_eq!(mock.name(), "Mock Connector");
    assert_eq!(mock.connector_type(), ConnectorType::Gmail);
    assert!(matches!(mock.mode(), ConnectorMode::Polling { .. }));
    assert!(mock.config_schema().is_object());
}

#[tokio::test]
async fn default_is_noop_success() {
    let mock = MockConnector::default();
    assert_eq!(mock.health().await.unwrap(), HealthStatus::Online);
    assert_eq!(
        mock.authenticate().await.unwrap(),
        ConnectorAuthState::Authenticated
    );
    let outcome = mock.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 0);
    assert!(mock.extract().await.unwrap().is_empty());
    mock.forget().await.unwrap();
}

// ---------------------------------------------------------------------------
// Config-driven identity from injected instance fields
// ---------------------------------------------------------------------------

#[test]
fn from_config_reads_injected_identity() {
    let config = json!({
        "__slug": "work-mail",
        "__ctype": ConnectorType::Calendar as i16,
        "__instance_id": 7,
    });
    let mock = MockConnector::from_config(config).unwrap();
    assert_eq!(mock.id(), "work-mail");
    assert_eq!(mock.connector_type(), ConnectorType::Calendar);
}

#[test]
fn from_config_defaults_identity_when_unset() {
    let mock = MockConnector::from_config(json!({})).unwrap();
    assert_eq!(mock.id(), "mock");
    assert_eq!(mock.connector_type(), ConnectorType::Gmail);
}

#[test]
fn from_config_display_name_defaults_to_slug() {
    let mock = MockConnector::from_config(json!({ "__slug": "cal1" })).unwrap();
    assert_eq!(mock.name(), "cal1");
}

#[test]
fn from_config_explicit_display_name_wins() {
    let mock = MockConnector::from_config(json!({
        "__slug": "cal1",
        "display_name": "Work Calendar",
    }))
    .unwrap();
    assert_eq!(mock.name(), "Work Calendar");
}

// ---------------------------------------------------------------------------
// Mode + cadence
// ---------------------------------------------------------------------------

#[test]
fn polling_mode_reads_interval_and_jitter() {
    let mock = MockConnector::from_config(json!({
        "mode": "polling",
        "interval_ms": 300,
        "jitter_ms": 25,
    }))
    .unwrap();
    match mock.mode() {
        ConnectorMode::Polling { interval, jitter } => {
            assert_eq!(interval, Duration::from_millis(300));
            assert_eq!(jitter, Duration::from_millis(25));
        }
        other => panic!("expected Polling, got {other:?}"),
    }
}

#[test]
fn push_mode_is_push() {
    let mock = MockConnector::from_config(json!({ "mode": "push", "interval_ms": 50 })).unwrap();
    assert_eq!(mock.mode(), ConnectorMode::Push);
}

// ---------------------------------------------------------------------------
// Canned facts: sync stages, extract drains
// ---------------------------------------------------------------------------

fn person_fact(subject: &str, rel: &str, object: &str, raw: &str) -> MockFactConfig {
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
        recurrence: mimir_knowledge::models::enums::RecurrenceType::None,
        requires_user_action: false,
        raw_reference: Some(raw.to_string()),
    }
}

#[tokio::test]
async fn sync_stages_canned_facts_and_extract_drains_them() {
    let config = json!({
        "__slug": "mock",
        "cursor": "v1",
        "facts": [
            person_fact("Alice", "works_at", "Acme", "m-1"),
            person_fact("Bob", "lives_in", "London", "m-2"),
        ],
    });
    let mock = MockConnector::from_config(config).unwrap();

    let outcome = mock.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 2);
    assert_eq!(outcome.new_cursor.as_deref(), Some("v1"));

    let facts: Vec<NormalizedFact> = mock.extract().await.unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].subject, "Alice");
    assert_eq!(facts[0].relationship_type, "works_at");
    assert_eq!(facts[0].object, "Acme");
    assert_eq!(facts[0].source_type, SourceType::Connector);
    assert_eq!(facts[0].raw_reference.as_deref(), Some("m-1"));
    assert!(!facts[0].is_correction);

    // The buffer is drained: a second extract yields nothing.
    assert!(mock.extract().await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_raw_reference_is_auto_generated_per_fact_index() {
    let mut fact = person_fact("Cara", "knows", "Dan", "x");
    fact.raw_reference = None;
    let config = json!({
        "__slug": "mock",
        "facts": [fact],
    });
    let mock = MockConnector::from_config(config).unwrap();
    mock.sync(SyncOptions::default()).await.unwrap();
    let facts = mock.extract().await.unwrap();
    assert_eq!(facts.len(), 1);
    // Auto-generated from the slug and the fact's position in the list.
    assert!(facts[0].raw_reference.as_deref().unwrap().contains("mock"));
}

#[tokio::test]
async fn batch_size_emits_facts_incrementally() {
    let facts: Vec<MockFactConfig> = (0..4)
        .map(|i| person_fact(&format!("P{i}"), "rel", "obj", &format!("m-{i}")))
        .collect();
    let config = json!({
        "__slug": "mock",
        "cursor": "c",
        "batch_size": 2u32,
        "facts": facts,
    });
    let mock = MockConnector::from_config(config).unwrap();

    // First sync: 2 facts.
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 2);
    assert_eq!(mock.extract().await.unwrap().len(), 2);
    // Second sync: the remaining 2.
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 2);
    assert_eq!(mock.extract().await.unwrap().len(), 2);
    // Third sync: exhausted.
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 0);
    assert!(mock.extract().await.unwrap().is_empty());
}

#[tokio::test]
async fn batch_size_advances_only_on_successful_syncs() {
    // Regression: the batch window must be keyed on *successful* syncs, not raw
    // call count, so failed/panicked cycles do not consume a window and drop
    // facts. fail_first=2 then success must still emit the first batch.
    let facts: Vec<MockFactConfig> = (0..4)
        .map(|i| person_fact(&format!("P{i}"), "rel", "obj", &format!("m-{i}")))
        .collect();
    let config = json!({
        "__slug": "mock",
        "cursor": "c",
        "batch_size": 2u32,
        "fail_first": 2,
        "facts": facts,
    });
    let mock = MockConnector::from_config(config).unwrap();

    // First two calls fail (do not consume a batch window).
    assert!(mock.sync(SyncOptions::default()).await.is_err());
    assert!(mock.sync(SyncOptions::default()).await.is_err());

    // Third call (first success) emits the first batch [0,2).
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 2);
    assert_eq!(mock.extract().await.unwrap().len(), 2);
    // Fourth call emits the remaining batch [2,4).
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 2);
    assert_eq!(mock.extract().await.unwrap().len(), 2);
    // Fifth call: exhausted.
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 0);
    assert!(mock.extract().await.unwrap().is_empty());
}

#[tokio::test]
async fn batch_size_advances_only_on_successful_syncs_after_panic() {
    // Same regression for the panic-injection path. The panic is run in a
    // spawned task (the way the supervisor catches it via `JoinError::is_panic`)
    // so no `futures::catch_unwind` dependency is needed.
    let facts: Vec<MockFactConfig> = (0..2)
        .map(|i| person_fact(&format!("Q{i}"), "rel", "obj", &format!("m-{i}")))
        .collect();
    let config = json!({
        "__slug": "mock",
        "cursor": "c",
        "batch_size": 1u32,
        "panic_first": 1,
        "facts": facts,
    });
    let mock = std::sync::Arc::new(MockConnector::from_config(config).unwrap());

    // First call panics (does not consume a batch window). Catch it in a
    // spawned task the way the supervisor does.
    let panicking = mock.clone();
    let handle = tokio::spawn(async move { panicking.sync(SyncOptions::default()).await });
    let join = handle.await;
    assert!(join.is_err(), "first sync should panic");
    assert!(join.unwrap_err().is_panic(), "JoinError should be a panic");

    // First success emits fact 0 (window not consumed by the panic).
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 1);
    assert_eq!(mock.extract().await.unwrap().len(), 1);
    // Second success emits fact 1.
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 1);
    assert_eq!(mock.extract().await.unwrap().len(), 1);
    // Exhausted.
    assert_eq!(mock.sync(SyncOptions::default()).await.unwrap().fetched, 0);
}

#[tokio::test]
async fn sensitive_fact_flag_is_carried_through() {
    let mut fact = person_fact("Secret", "earns", "100000", "s-1");
    fact.is_sensitive = true;
    let config = json!({ "__slug": "mock", "facts": [fact] });
    let mock = MockConnector::from_config(config).unwrap();
    mock.sync(SyncOptions::default()).await.unwrap();
    let facts = mock.extract().await.unwrap();
    assert!(facts[0].is_sensitive);
}

// ---------------------------------------------------------------------------
// Health / auth / failure / panic knobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_is_configurable() {
    let mock = MockConnector::from_config(json!({ "health": "auth_expired" })).unwrap();
    assert_eq!(mock.health().await.unwrap(), HealthStatus::AuthExpired);
}

#[tokio::test]
async fn authenticate_is_configurable() {
    let mock = MockConnector::from_config(json!({ "auth_state": "Unauthenticated" })).unwrap();
    assert_eq!(
        mock.authenticate().await.unwrap(),
        ConnectorAuthState::Unauthenticated
    );
}

#[tokio::test]
async fn fail_first_then_succeed() {
    let config = json!({ "__slug": "mock", "fail_first": 2, "cursor": "ok" });
    let mock = MockConnector::from_config(config).unwrap();
    assert!(mock.sync(SyncOptions::default()).await.is_err());
    assert!(mock.sync(SyncOptions::default()).await.is_err());
    let outcome = mock.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.new_cursor.as_deref(), Some("ok"));
}

#[tokio::test]
async fn always_fail_never_succeeds() {
    let mock = MockConnector::from_config(json!({ "always_fail": true })).unwrap();
    for _ in 0..3 {
        assert!(mock.sync(SyncOptions::default()).await.is_err());
    }
}

// Panic injection (`panic_first`) is exercised end-to-end by the supervisor
// integration test (`task_panic_is_recovered_then_succeeds`), which catches the
// panic via `JoinError::is_panic` — no `futures::catch_unwind` dependency needed
// here.

// ---------------------------------------------------------------------------
// Push mode paces via an internal sleep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_mode_sync_self_paces_then_emits() {
    let config = json!({
        "__slug": "pushy",
        "mode": "push",
        "interval_ms": 40,
        "cursor": "pc",
        "facts": [person_fact("Eve", "rel", "obj", "e-1")],
    });
    let mock = MockConnector::from_config(config).unwrap();
    let start = tokio::time::Instant::now();
    let outcome = mock.sync(SyncOptions::default()).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(40));
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("pc"));
    assert_eq!(mock.extract().await.unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// SyncOptions observation (F9-style concurrency instrumentation)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recorder_observes_sync_options() {
    let recorder = Arc::new(MockSyncRecorder::default());
    let config = json!({ "__slug": "mock", "cursor": "c", "sync_delay_ms": 20 });
    let mock = MockConnector::from_config(config)
        .unwrap()
        .with_recorder(recorder.clone());

    let opts = SyncOptions {
        full: true,
        since: Some(Duration::from_secs(60)),
    };
    mock.sync(opts).await.unwrap();
    assert_eq!(recorder.len(), 1);
    let last = recorder.last().unwrap();
    assert!(last.full);
    assert_eq!(last.since, Some(Duration::from_secs(60)));
    assert_eq!(recorder.max_concurrent(), 1);
}

// ---------------------------------------------------------------------------
// Factory + registry wiring
// ---------------------------------------------------------------------------

#[test]
fn factory_builds_configured_mock() {
    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Gmail, "mock", MockConnectorFactory)
        .unwrap();
    let config = json!({
        "__slug": "gmail-mock",
        "__ctype": ConnectorType::Gmail as i16,
        "cursor": "f1",
        "facts": [person_fact("Alice", "works_at", "Acme", "m-1")],
    });
    let connector = registry
        .create(ConnectorType::Gmail, "mock", config)
        .unwrap();
    assert_eq!(connector.id(), "gmail-mock");
    assert_eq!(connector.connector_type(), ConnectorType::Gmail);
}

#[test]
fn factory_invalid_config_returns_config_error() {
    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Gmail, "mock", MockConnectorFactory)
        .unwrap();
    // `interval_ms` expects a number; a string is invalid.
    let err = registry
        .create(
            ConnectorType::Gmail,
            "mock",
            json!({ "interval_ms": "oops" }),
        )
        .err()
        .unwrap();
    assert!(matches!(err, ConnectorError::Config(_)));
}

// ---------------------------------------------------------------------------
// config_schema describes the surface
// ---------------------------------------------------------------------------

#[test]
fn config_schema_describes_mode_and_facts() {
    let schema = MockConnector::default().config_schema();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .unwrap();
    assert!(props.contains_key("mode"));
    assert!(props.contains_key("interval_ms"));
    assert!(props.contains_key("facts"));
    assert!(props.contains_key("health"));
}
