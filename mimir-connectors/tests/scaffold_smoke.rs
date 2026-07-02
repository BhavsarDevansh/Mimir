//! Scaffolding smoke test for the `mimir-connectors` crate (issue #178 / F1).
//!
//! This is the F1 anchor test: it referenced `mimir_connectors` *before* the
//! crate existed, so it failed to compile. Now that the crate is scaffolded it
//! passes, proving the trait/registry/mock stubs compile and are usable under
//! every feature combination. Real behavioural tests arrive with F6/F7/F13.

use mimir_connectors::{Connector, ConnectorRegistry, MockConnector};

#[test]
fn registry_starts_empty() {
    let registry = ConnectorRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn mock_connector_reports_identity() {
    let mock = MockConnector;

    assert_eq!(mock.id(), "mock");
    assert_eq!(mock.name(), "Mock Connector");

    // Trait-object coercion proves the placeholder trait is object-safe.
    let dyn_ref: &dyn Connector = &mock;
    assert_eq!(dyn_ref.id(), "mock");
}
