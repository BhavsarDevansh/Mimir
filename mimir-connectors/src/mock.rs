//! Always-compiled mock connector for testing.
//!
//! Placeholder scaffold. The full configurable in-memory test harness — which
//! emits canned normalized facts through the knowledge-graph pipeline — is
//! owned by Phase 3 issue **F13**. This minimal implementation exists so the
//! always-compiled mock path stays valid under every feature combination,
//! including `--no-default-features`.

use crate::connector::Connector;

/// No-op mock connector used by integration tests.
///
/// F13 will expand this into a configurable harness; for now it implements the
/// placeholder [`Connector`] identity accessors.
#[derive(Debug, Default)]
pub struct MockConnector;

impl Connector for MockConnector {
    fn id(&self) -> &str {
        "mock"
    }

    fn name(&self) -> &str {
        "Mock Connector"
    }
}
