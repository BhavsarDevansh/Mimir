//! F7 behavioural tests (issue #184): `ConnectorRegistry` multi-backend
//! factory dispatch.
//!
//! These reference the registry/factory API *before* it exists, so they fail
//! to compile until F7 lands — the TDD anchor.
//!
//! Design (locked):
//! - The registry maps `(ConnectorType, backend) -> ConnectorFactory`. A
//!   connector *type* is the provenance/reliability axis; a *backend* is the
//!   provider implementation chosen per instance. New backends register a new
//!   factory — no schema change.
//! - `register` is `&self` (interior mutability, matching `ToolRegistry`) so
//!   the registry can be shared in `AppState` behind `Arc`.
//! - `create` returns `Arc<dyn Connector>` (the shared-storage shape used by
//!   the supervisor, F8), not `Box`.
//! - Reliability stays per-type: confidence is derived from `connector_type()`
//!   alone, never from `backend`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use mimir_connectors::{
    Connector, ConnectorContext, ConnectorError, ConnectorFactory, ConnectorRegistry,
    FnConnectorFactory, SyncOptions,
};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};

/// Minimal configurable connector used only by these tests. It encodes the
/// factory's chosen `backend` tag and the config-supplied slug into `id()` so
/// dispatch and config-passthrough are observable through the trait surface.
struct FakeConnector {
    id: String,
    name: String,
    connector_type: ConnectorType,
}

#[async_trait]
impl Connector for FakeConnector {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn connector_type(&self) -> ConnectorType {
        self.connector_type
    }
    fn mode(&self) -> mimir_connectors::ConnectorMode {
        mimir_connectors::ConnectorMode::Polling {
            interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
        }
    }
    fn config_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        Ok(ConnectorAuthState::Authenticated)
    }
    async fn health(&self) -> Result<mimir_connectors::HealthStatus, ConnectorError> {
        Ok(mimir_connectors::HealthStatus::Online)
    }
    async fn sync(
        &self,
        _options: SyncOptions,
    ) -> Result<mimir_connectors::SyncOutcome, ConnectorError> {
        Ok(mimir_connectors::SyncOutcome {
            fetched: 0,
            new_cursor: None,
            fetched_at: chrono::Utc::now(),
        })
    }
    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        Ok(Vec::new())
    }
    async fn forget(&self) -> Result<(), ConnectorError> {
        Ok(())
    }
}

/// Build a factory that produces a `FakeConnector` whose `id()` is
/// `<backend_tag>:<config.slug>`, proving both dispatch and config passthrough.
fn fake_factory(backend_tag: &'static str, ct: ConnectorType) -> FnConnectorFactory {
    FnConnectorFactory::new(move |config: serde_json::Value, _ctx: &ConnectorContext| {
        let slug = config
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default");
        Ok(Arc::new(FakeConnector {
            id: format!("{backend_tag}:{slug}"),
            name: format!("{backend_tag} backend"),
            connector_type: ct,
        }) as Arc<dyn Connector>)
    })
}

// ---------------------------------------------------------------------------
// Registration + lookup
// ---------------------------------------------------------------------------

#[test]
fn register_then_factory_lookup_succeeds() {
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "mock-imap",
            fake_factory("imap", ConnectorType::Gmail),
        )
        .unwrap();

    assert!(registry.is_registered(ConnectorType::Gmail, "mock-imap"));
    assert!(!registry.is_registered(ConnectorType::Gmail, "mock-graph"));
    assert!(
        registry
            .factory(ConnectorType::Gmail, "mock-imap")
            .is_some()
    );
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn backends_for_type_lists_all_registered_backends() {
    let registry = ConnectorRegistry::new();
    // Registered in non-alphabetical order; backends_for() must return a
    // deterministic sorted list so discovery never depends on HashMap
    // iteration order.
    for backend in ["z-backend", "a-backend", "m-backend"] {
        registry
            .register(
                ConnectorType::Gmail,
                backend,
                fake_factory(backend, ConnectorType::Gmail),
            )
            .unwrap();
    }

    assert_eq!(
        registry.backends_for(ConnectorType::Gmail),
        vec!["a-backend", "m-backend", "z-backend"]
    );
}

#[test]
fn pairs_lists_all_registered_pairs_sorted_by_type_then_backend() {
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "imap",
            fake_factory("imap", ConnectorType::Gmail),
        )
        .unwrap();
    registry
        .register(
            ConnectorType::Photos,
            "local",
            fake_factory("local", ConnectorType::Photos),
        )
        .unwrap();
    registry
        .register(
            ConnectorType::Calendar,
            "caldav",
            fake_factory("caldav", ConnectorType::Calendar),
        )
        .unwrap();
    registry
        .register(
            ConnectorType::Gmail,
            "graph",
            fake_factory("graph", ConnectorType::Gmail),
        )
        .unwrap();

    // Registered in arbitrary order; pairs() must return a deterministic
    // sorted-by-(type, backend) list for stable wire output and table UX.
    let pairs = registry.pairs();
    assert_eq!(
        pairs,
        vec![
            (ConnectorType::Calendar, "caldav".to_string()),
            (ConnectorType::Gmail, "graph".to_string()),
            (ConnectorType::Gmail, "imap".to_string()),
            (ConnectorType::Photos, "local".to_string()),
        ]
    );
}

// ---------------------------------------------------------------------------
// Multi-backend dispatch (core acceptance)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_backends_under_same_type_coexist_and_dispatch_correctly() {
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "mock-imap",
            fake_factory("imap", ConnectorType::Gmail),
        )
        .unwrap();
    registry
        .register(
            ConnectorType::Gmail,
            "mock-graph",
            fake_factory("graph", ConnectorType::Gmail),
        )
        .unwrap();

    let imap = registry
        .create(ConnectorType::Gmail, "mock-imap", json!({"slug": "acct1"}))
        .unwrap();
    let graph = registry
        .create(ConnectorType::Gmail, "mock-graph", json!({"slug": "acct1"}))
        .unwrap();

    // Same config, different backends -> different connector instances.
    assert_eq!(imap.id(), "imap:acct1");
    assert_eq!(graph.id(), "graph:acct1");

    // Both report the same provenance/reliability axis (the type), not backend.
    assert_eq!(imap.connector_type(), ConnectorType::Gmail);
    assert_eq!(graph.connector_type(), ConnectorType::Gmail);
}

#[tokio::test]
async fn create_passes_config_to_factory() {
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "mock-imap",
            fake_factory("imap", ConnectorType::Gmail),
        )
        .unwrap();

    let connector = registry
        .create(
            ConnectorType::Gmail,
            "mock-imap",
            json!({"slug": "work-mail"}),
        )
        .unwrap();
    assert_eq!(connector.id(), "imap:work-mail");
}

#[tokio::test]
async fn same_backend_name_under_different_types_dispatches_independently() {
    // The key is the (type, backend) pair, not backend alone.
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "mock",
            fake_factory("gmail-mock", ConnectorType::Gmail),
        )
        .unwrap();
    registry
        .register(
            ConnectorType::Calendar,
            "mock",
            fake_factory("cal-mock", ConnectorType::Calendar),
        )
        .unwrap();

    let mail = registry
        .create(ConnectorType::Gmail, "mock", json!({}))
        .unwrap();
    let cal = registry
        .create(ConnectorType::Calendar, "mock", json!({}))
        .unwrap();

    assert_eq!(mail.id(), "gmail-mock:default");
    assert_eq!(cal.id(), "cal-mock:default");
    assert_eq!(mail.connector_type(), ConnectorType::Gmail);
    assert_eq!(cal.connector_type(), ConnectorType::Calendar);
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn create_unknown_backend_returns_backend_not_found() {
    let registry = ConnectorRegistry::new();
    // `err().unwrap()` instead of `unwrap_err()`: the Ok type
    // (`Arc<dyn Connector>`) is not `Debug`.
    let err = registry
        .create(ConnectorType::Gmail, "nope", json!({}))
        .err()
        .unwrap();
    assert!(matches!(
        err,
        ConnectorError::BackendNotFound {
            connector_type: ConnectorType::Gmail,
            ref backend
        } if backend == "nope"
    ));
}

#[test]
fn register_duplicate_returns_backend_already_registered() {
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Gmail,
            "mock-imap",
            fake_factory("imap", ConnectorType::Gmail),
        )
        .unwrap();
    let err = registry
        .register(
            ConnectorType::Gmail,
            "mock-imap",
            fake_factory("imap2", ConnectorType::Gmail),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorError::BackendAlreadyRegistered {
            connector_type: ConnectorType::Gmail,
            ref backend
        } if backend == "mock-imap"
    ));
}

#[test]
fn factory_trait_is_object_safe() {
    // Factories are stored as Arc<dyn ConnectorFactory> inside the registry.
    let _factory: Arc<dyn ConnectorFactory> = Arc::new(fake_factory("imap", ConnectorType::Gmail));
}
