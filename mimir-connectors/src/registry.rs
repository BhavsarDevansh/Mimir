//! `ConnectorRegistry` and multi-backend factory dispatch (Phase 3 F7 /
//! issue #184).
//!
//! The registry maps `(connector_type, backend) -> ConnectorFactory`. A
//! connector *type* (`Gmail` / `Calendar` / `Photos` / …) is the provenance
//! and reliability axis — fixed and seeded. A *backend* (`imap`, `caldav`,
//! `local-fs`, …) is the provider implementation chosen per instance and
//! persisted as the `backend` column on the `connectors` table (F2). Adding a
//! new backend is a new `register` call — no schema change.
//!
//! # Reliability stays per-type
//!
//! Confidence for connector-sourced facts is
//! `confidence::initial(SourceType::Connector, connector_type)` (see
//! `mimir-knowledge`), keyed on the type axis only. The registry never
//! branches reliability on `backend`; the same `connector_type()` is reported
//! regardless of which backend constructed the instance.
//!
//! # Concurrency
//!
//! Following the workspace `ToolRegistry` / `SkillRegistry` pattern,
//! registration uses interior mutability (`RwLock`) with a `&self` receiver, so
//! a registry shared in `AppState` behind `Arc` can be populated at startup
//! and queried concurrently at runtime. `register` fails loud on a duplicate
//! `(type, backend)` to surface accidental re-registration rather than
//! silently shadowing a previously-registered backend.
//!
//! # Poison handling
//!
//! A poisoned `RwLock` means a task panicked while holding the write lock — the
//! map may be partially mutated and the state is unrecoverable. Every accessor
//! therefore propagates poison by panicking (via private `read`/`write` helpers
//! that `.expect`), matching the workspace `ToolRegistry` convention, so the
//! registry never reports contradictory state after a panic.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use mimir_knowledge::models::enums::ConnectorType;

use crate::connector::{Connector, ConnectorError, ConnectorFactory};

/// Registry mapping `(connector_type, backend)` pairs to their
/// [`ConnectorFactory`] (Phase 3 F7 / issue #184).
///
/// One factory per backend; many backends may coexist under a single
/// connector type. The registry constructs instances via [`create`]
/// (`Self::create`); it does **not** own running instances — that is the
/// supervisor's job (F8).
///
/// [`create`]: ConnectorRegistry::create
pub struct ConnectorRegistry {
    factories: RwLock<HashMap<(ConnectorType, String), Arc<dyn ConnectorFactory>>>,
}

impl std::fmt::Debug for ConnectorRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn ConnectorFactory` is not `Debug`, so report the registered
        // (type, backend) keys rather than recursing into the trait objects.
        let keys = self.read().keys().cloned().collect::<Vec<_>>();
        f.debug_struct("ConnectorRegistry")
            .field("len", &keys.len())
            .field("entries", &keys)
            .finish()
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectorRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            factories: RwLock::new(HashMap::new()),
        }
    }

    /// Acquire a read lock, panicking on poison.
    ///
    /// Poison means a task panicked mid-write, leaving potentially
    /// partially-mutated state — unrecoverable, so it is propagated rather
    /// than silently degraded. This keeps every accessor consistent: none
    /// report contradictory (e.g. "empty yet locked") state after a panic.
    fn read(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, HashMap<(ConnectorType, String), Arc<dyn ConnectorFactory>>>
    {
        self.factories
            .read()
            .expect("ConnectorRegistry lock poisoned")
    }

    /// Acquire a write lock, panicking on poison (see [`read`](Self::read)).
    fn write(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, HashMap<(ConnectorType, String), Arc<dyn ConnectorFactory>>>
    {
        self.factories
            .write()
            .expect("ConnectorRegistry lock poisoned")
    }

    /// Number of registered `(type, backend)` factories.
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// Whether no factories are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Register a factory for a `(connector_type, backend)` pair.
    ///
    /// Returns [`ConnectorError::BackendAlreadyRegistered`] if a factory is
    /// already registered for the same pair, so accidental re-registration
    /// fails loud instead of shadowing the existing backend.
    ///
    /// Accepts any concrete [`ConnectorFactory`] value; it is stored as the
    /// shared `Arc<dyn ConnectorFactory>` shape. Pre-built trait objects can be
    /// registered with [`register_arc`](Self::register_arc).
    pub fn register<F>(
        &self,
        connector_type: ConnectorType,
        backend: impl Into<String>,
        factory: F,
    ) -> Result<(), ConnectorError>
    where
        F: ConnectorFactory + 'static,
    {
        self.register_arc(connector_type, backend, Arc::new(factory))
    }

    /// Register an already-constructed `Arc<dyn ConnectorFactory>`.
    ///
    /// Use [`register`](Self::register) for owned factory values; this variant
    /// covers cases where a factory trait object is built and shared upstream.
    pub fn register_arc(
        &self,
        connector_type: ConnectorType,
        backend: impl Into<String>,
        factory: Arc<dyn ConnectorFactory>,
    ) -> Result<(), ConnectorError> {
        let backend = backend.into();
        let key = (connector_type, backend.clone());
        let mut guard = self.write();
        if guard.contains_key(&key) {
            return Err(ConnectorError::BackendAlreadyRegistered {
                connector_type,
                backend,
            });
        }
        guard.insert(key, factory);
        Ok(())
    }

    /// Whether a factory is registered for the given `(type, backend)`.
    pub fn is_registered(&self, connector_type: ConnectorType, backend: &str) -> bool {
        self.read()
            .contains_key(&(connector_type, backend.to_string()))
    }

    /// Clone out the factory registered for `(type, backend)`, if any.
    pub fn factory(
        &self,
        connector_type: ConnectorType,
        backend: &str,
    ) -> Option<Arc<dyn ConnectorFactory>> {
        self.read()
            .get(&(connector_type, backend.to_string()))
            .cloned()
    }

    /// List the backend names registered under a connector type, in arbitrary
    /// order. Useful for the `connector add` flow's backend discovery.
    pub fn backends_for(&self, connector_type: ConnectorType) -> Vec<String> {
        self.read()
            .keys()
            .filter(|(ct, _)| *ct == connector_type)
            .map(|(_, b)| b.clone())
            .collect::<Vec<_>>()
    }

    /// All connector types that have at least one registered backend.
    pub fn registered_types(&self) -> Vec<ConnectorType> {
        self.read()
            .keys()
            .map(|(ct, _)| *ct)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    }

    /// Construct a connector instance for `(type, backend)` from its config.
    ///
    /// Looks up the registered factory and delegates to
    /// [`ConnectorFactory::create`]. Returns
    /// [`ConnectorError::BackendNotFound`] when no factory is registered for
    /// the requested pair. `config` is the deserialised `config_json` of the
    /// `connectors` row.
    pub fn create(
        &self,
        connector_type: ConnectorType,
        backend: &str,
        config: serde_json::Value,
    ) -> Result<Arc<dyn Connector>, ConnectorError> {
        // One read-lock acquisition and one key allocation for the lookup.
        let backend = backend.to_string();
        let key = (connector_type, backend.clone());
        let factory = self
            .read()
            .get(&key)
            .cloned()
            .ok_or(ConnectorError::BackendNotFound {
                connector_type,
                backend,
            })?;
        factory.create(config)
    }
}

/// Type of the closure stored inside [`FnConnectorFactory`].
type FactoryFn =
    Arc<dyn Fn(serde_json::Value) -> Result<Arc<dyn Connector>, ConnectorError> + Send + Sync>;

/// A [`ConnectorFactory`] backed by an `Fn(serde_json::Value) -> Result<…>`
/// closure.
///
/// Convenient for registering simple backends and for tests without defining a
/// dedicated factory struct. The closure must be `Send + Sync + 'static`.
#[derive(Clone)]
pub struct FnConnectorFactory {
    f: FactoryFn,
}

impl FnConnectorFactory {
    /// Wrap a closure as a [`ConnectorFactory`].
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(serde_json::Value) -> Result<Arc<dyn Connector>, ConnectorError>
            + Send
            + Sync
            + 'static,
    {
        Self { f: Arc::new(f) }
    }
}

impl ConnectorFactory for FnConnectorFactory {
    fn create(&self, config: serde_json::Value) -> Result<Arc<dyn Connector>, ConnectorError> {
        (self.f)(config)
    }
}

impl std::fmt::Debug for FnConnectorFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnConnectorFactory").finish_non_exhaustive()
    }
}
