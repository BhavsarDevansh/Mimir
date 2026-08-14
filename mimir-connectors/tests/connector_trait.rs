//! F6 behavioural tests (issue #183): the runtime `Connector` trait and its
//! supporting data types. These reference the full trait surface *before* it
//! is implemented, so they fail to compile until F6 lands — the TDD anchor.
//!
//! Design (locked):
//! - Two-step ingestion: `sync()` fetches raw items into the connector's own
//!   buffer; `extract()` drains them into `Vec<NormalizedFact>`. The
//!   *supervisor* (F8) calls `mimir_knowledge::normalize::normalize_and_insert`
//!   — the connector itself is DB-free, so the trait takes no
//!   `&KnowledgeGraph`.
//! - `HealthStatus` is a transient runtime probe, renamed to disambiguate from
//!   the persisted `ConnectorStatus` / `ConnectorAuthState` enums.
//! - `act()` is optional write-back with a default `UnsupportedAction` impl.

use std::sync::Arc;
use std::time::Duration;

use mimir_connectors::{
    ActionResult, Connector, ConnectorAction, ConnectorError, ConnectorMode, HealthStatus,
    MockConnector, SyncOptions, SyncOutcome,
};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

// ---------------------------------------------------------------------------
// Identity + declarative accessors
// ---------------------------------------------------------------------------

#[test]
fn mock_reports_identity_type_and_mode() {
    let mock = MockConnector::default();

    assert_eq!(mock.id(), "mock");
    assert_eq!(mock.name(), "Mock Connector");
    assert_eq!(mock.connector_type(), ConnectorType::Gmail);
    assert!(mock.config_schema().is_object());
}

#[test]
fn connector_mode_distinguishes_polling_and_push() {
    // F8 acceptance: the supervisor must be able to tell polling from push.
    let polling = ConnectorMode::Polling {
        interval: Duration::from_secs(300),
        jitter: Duration::from_secs(30),
    };
    assert!(matches!(polling, ConnectorMode::Polling { .. }));

    let push = ConnectorMode::Push;
    assert!(matches!(push, ConnectorMode::Push));

    // Different variants are not equal.
    assert_ne!(polling, push);
}

// ---------------------------------------------------------------------------
// Health — renamed variants disambiguate from persisted lifecycle enums
// ---------------------------------------------------------------------------

#[test]
fn health_status_renamed_variants_exist() {
    // Five transient probe outcomes; none reuse persisted-enum names.
    let _ = HealthStatus::Online;
    let _ = HealthStatus::Offline;
    let _ = HealthStatus::Degraded;
    let _ = HealthStatus::AuthExpired;
    let _ = HealthStatus::NotConfigured;
}

// ---------------------------------------------------------------------------
// SyncOptions / SyncOutcome / ConnectorAction / ActionResult construction
// ---------------------------------------------------------------------------

#[test]
fn sync_options_full_and_incremental() {
    let full = SyncOptions {
        full: true,
        since: None,
    };
    assert!(full.full);

    let incremental = SyncOptions {
        full: false,
        since: Some(Duration::from_secs(86_400)),
    };
    assert!(!incremental.full);
    assert_eq!(incremental.since, Some(Duration::from_secs(86_400)));
}

#[test]
fn sync_outcome_carries_cursor_and_counts() {
    let outcome = SyncOutcome {
        fetched: 42,
        new_cursor: Some("uid:1234".to_string()),
        fetched_at: chrono::Utc::now(),
    };
    assert_eq!(outcome.fetched, 42);
    assert_eq!(outcome.new_cursor.as_deref(), Some("uid:1234"));
}

#[test]
fn connector_action_and_result_roundtrip() {
    let action = ConnectorAction {
        kind: "create_event".to_string(),
        payload: serde_json::json!({"title": "Dentist"}),
    };
    assert_eq!(action.kind, "create_event");

    let result = ActionResult {
        success: true,
        native_id: Some("evt-9".to_string()),
        message: None,
    };
    assert!(result.success);
    assert_eq!(result.native_id.as_deref(), Some("evt-9"));
}

// ---------------------------------------------------------------------------
// ConnectorError
// ---------------------------------------------------------------------------

#[test]
fn connector_error_variants_display() {
    assert!(
        format!("{}", ConnectorError::Authentication("bad token".into())).contains("bad token")
    );
    assert!(!format!("{}", ConnectorError::NotAuthenticated).is_empty());
    assert!(format!("{}", ConnectorError::Network("timeout".into())).contains("timeout"));
    assert!(format!("{}", ConnectorError::Config("missing host".into())).contains("missing host"));
    assert!(format!("{}", ConnectorError::Parse("bad mime".into())).contains("bad mime"));
    assert!(format!("{}", ConnectorError::UnsupportedAction("x".into())).contains("x"));
    assert!(!format!("{}", ConnectorError::Other("boom".into())).is_empty());
}

// ---------------------------------------------------------------------------
// Async trait surface (needs a tokio runtime)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_health_is_online() {
    let mock = MockConnector::default();
    assert_eq!(mock.health().await.unwrap(), HealthStatus::Online);
}

#[tokio::test]
async fn mock_authenticate_returns_authenticated() {
    let mock = MockConnector::default();
    assert_eq!(
        mock.authenticate().await.unwrap(),
        ConnectorAuthState::Authenticated
    );
}

#[tokio::test]
async fn mock_sync_then_extract_yields_empty_normalized_facts() {
    let mock = MockConnector::default();
    let outcome = mock
        .sync(SyncOptions {
            full: false,
            since: None,
        })
        .await
        .unwrap();
    assert_eq!(outcome.fetched, 0);

    let facts: Vec<NormalizedFact> = mock.extract().await.unwrap();
    assert!(facts.is_empty());
}

#[tokio::test]
async fn mock_act_default_is_unsupported() {
    let mock = MockConnector::default();
    let action = ConnectorAction {
        kind: "create_event".to_string(),
        payload: serde_json::json!({}),
    };
    let err = mock.act(action).await.unwrap_err();
    assert!(matches!(err, ConnectorError::UnsupportedAction(_)));
}

#[tokio::test]
async fn mock_forget_succeeds() {
    let mock = MockConnector::default();
    mock.forget().await.unwrap();
}

#[tokio::test]
async fn extract_deletions_defaults_to_empty() {
    // The tombstone report (issue #247) is an optional trait method: a
    // connector without staged deletions reports no removals, so connectors
    // without server-side deletions keep the no-op behaviour. The
    // acknowledgement half defaults to a no-op too (nothing is buffered).
    let mock = MockConnector::default();
    assert!(
        mock.extract_deletions().await.unwrap().is_empty(),
        "connector without configured deletions reports none"
    );
    mock.acknowledge_deletions(&["never-staged".to_string()])
        .await
        .unwrap();
    assert!(mock.extract_deletions().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Object safety: the trait must work behind Arc<dyn Connector>
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trait_is_object_safe() {
    // A shared `Arc<dyn Connector>` is the documented storage shape for the
    // registry (F7) and supervisor (F8). Every async method takes `&self`, so
    // the whole surface — including sync/extract/authenticate/forget — is
    // callable through the shared reference without interior mutability at
    // the *storage* layer. (Connectors that need mutable state own it behind
    // their own interior mutability; the mock needs none.)
    let mock: Arc<dyn Connector> = Arc::new(MockConnector::default());

    // Sync accessors through the trait object.
    assert_eq!(mock.id(), "mock");
    assert_eq!(mock.name(), "Mock Connector");
    assert_eq!(mock.connector_type(), ConnectorType::Gmail);
    assert!(matches!(mock.mode(), ConnectorMode::Polling { .. }));
    assert!(mock.config_schema().is_object());

    // Async accessors — including the ones that used to be `&mut self` — must
    // be callable through the shared `Arc<dyn Connector>`.
    assert_eq!(mock.health().await.unwrap(), HealthStatus::Online);
    assert_eq!(
        mock.authenticate().await.unwrap(),
        ConnectorAuthState::Authenticated
    );
    let outcome = mock
        .sync(SyncOptions {
            full: false,
            since: None,
        })
        .await
        .unwrap();
    assert_eq!(outcome.fetched, 0);
    assert!(mock.extract().await.unwrap().is_empty());
    mock.forget().await.unwrap();

    // The default `act()` implementation is also callable through the trait object.
    let err = mock
        .act(ConnectorAction {
            kind: "create_event".to_string(),
            payload: serde_json::json!({}),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::UnsupportedAction(_)));

    // The shared reference is still usable after the calls (not consumed).
    assert_eq!(mock.id(), "mock");
}
