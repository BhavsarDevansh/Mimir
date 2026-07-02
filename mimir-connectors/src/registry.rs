//! `ConnectorRegistry` and multi-backend factory dispatch.
//!
//! Placeholder scaffold. The full registry — registration by backend, lookup,
//! factory dispatch, and CRUD/status surface — is owned by Phase 3 issue
//! **F7**. For now this exposes construction and a length check, which is
//! sufficient to keep the crate compiling and to anchor the scaffolding smoke
//! test.

use std::sync::Arc;

use crate::connector::Connector;

/// Registry of active connector instances held behind [`Arc<dyn Connector>`].
///
/// The real registration/lookup/factory-dispatch API is filled in by F7.
#[derive(Default)]
pub struct ConnectorRegistry {
    connectors: Vec<Arc<dyn Connector>>,
}

impl std::fmt::Debug for ConnectorRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn Connector` is not required to be `Debug`, so report the count
        // rather than recursing into the trait objects.
        f.debug_struct("ConnectorRegistry")
            .field("len", &self.connectors.len())
            .finish()
    }
}

impl ConnectorRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            connectors: Vec::new(),
        }
    }

    /// Number of registered connectors.
    pub fn len(&self) -> usize {
        self.connectors.len()
    }

    /// Whether no connectors are registered.
    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }
}
