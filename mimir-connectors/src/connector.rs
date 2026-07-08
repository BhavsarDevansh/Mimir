//! Runtime [`Connector`] trait and its supporting data types (Phase 3 F6 /
//! issue #183).
//!
//! This is the contract every service-ingestion worker implements. Connectors
//! are background sync workers that fetch data from an external service,
//! normalize it, and hand it to the knowledge-graph pipeline — they are **not**
//! a parallel track.
//!
//! # Ingestion model (locked)
//!
//! Ingestion is a two-step process owned by the connector, with the
//! *supervisor* (F8) performing the database insert:
//!
//! 1. [`Connector::sync`] fetches raw items from the service into the
//!    connector's own internal buffer and returns a [`SyncOutcome`] (item
//!    count + new sync cursor).
//! 2. [`Connector::extract`] drains that buffer into a `Vec<NormalizedFact>`
//!    ([`mimir_knowledge::normalize::NormalizedFact`]) — typed, parsed facts
//!    with entity *types* but **unresolved** entity ids.
//! 3. The supervisor builds a `Provenance::connector(instance_id, type, method)`
//!    and calls `mimir_knowledge::normalize::normalize_and_insert` to resolve
//!    entities, assign confidence, run the sensitivity gate, and insert
//!    (inheriting corroboration / supersession / inference).
//!
//! Because the connector never touches the database, the trait takes no
//! `&KnowledgeGraph` parameter and the crate stays `sqlx`-free. This also
//! keeps connectors unit-testable without a live knowledge graph (F13 mock).
//!
//! # Object safety
//!
//! The trait is used as `Arc<dyn Connector>` by the registry (F7) and the
//! supervisor (F8). Native `async fn` in traits is not dyn-compatible
//! (`error[E0038]`), so [`async_trait`] is used with the default `Send` bound
//! so `dyn Connector` is usable across multi-threaded tasks.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors raised by a connector operation.
///
/// Deliberately does **not** wrap [`mimir_knowledge::KnowledgeError`]: the
/// connector does not perform database inserts (the supervisor owns
/// `normalize_and_insert`), so a connector call never surfaces a knowledge-graph
/// error directly. A `Knowledge` variant can be added later if a backend ever
/// calls the facade itself.
#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    /// Authentication handshake failed (rejected credentials, OAuth error).
    #[error("authentication failed: {0}")]
    Authentication(String),

    /// The connector has no valid credentials and cannot sync yet.
    #[error("connector is not authenticated")]
    NotAuthenticated,

    /// Network-level failure reaching the service (timeout, DNS, connection).
    #[error("network error: {0}")]
    Network(String),

    /// Connector configuration is missing or invalid.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// A service item could not be parsed into a normalized fact.
    #[error("failed to parse service data: {0}")]
    Parse(String),

    /// The connector does not implement the requested write-back action.
    ///
    /// Returned by the default [`Connector::act`] implementation.
    #[error("action `{0}` is not supported by this connector")]
    UnsupportedAction(String),

    /// Local I/O failure (e.g. reading a photo file from disk).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// No factory is registered for the requested `(connector_type, backend)`
    /// pair. Raised by [`crate::registry::ConnectorRegistry::create`] when the
    /// registry cannot dispatch the requested backend.
    #[error("no connector factory for {connector_type:?} backend `{backend}`")]
    BackendNotFound {
        connector_type: mimir_knowledge::models::enums::ConnectorType,
        backend: String,
    },

    /// A factory is already registered for the requested
    /// `(connector_type, backend)` pair. Raised by
    /// [`crate::registry::ConnectorRegistry::register`] on a duplicate
    /// registration so accidental re-registration fails loud rather than
    /// silently shadowing a previously-registered backend.
    #[error("connector factory already registered for {connector_type:?} backend `{backend}`")]
    BackendAlreadyRegistered {
        connector_type: mimir_knowledge::models::enums::ConnectorType,
        backend: String,
    },

    /// Any other connector-specific failure not covered above.
    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------------------
// Mode + sync options + outcome
// ---------------------------------------------------------------------------

/// How the supervisor should run a connector (decision D).
///
/// `Polling` connectors are polled on a fixed interval with random jitter to
/// avoid thundering-herd syncs across instances. `Push` connectors receive
/// events from the service (IMAP IDLE, a file watcher) and are not polled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorMode {
    Polling {
        /// Base time between syncs.
        interval: Duration,
        /// Random jitter applied on top of `interval` to spread out syncs.
        jitter: Duration,
    },
    Push,
}

/// Options passed to [`Connector::sync`].
///
/// `full` requests a complete re-fetch (ignoring any persisted cursor);
/// otherwise the connector syncs incrementally using its opaque cursor
/// (persisted in the `connectors.sync_cursor` column, not here). `since` is an
/// optional relative time-window hint (e.g. "only the last 24 h").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOptions {
    /// `true` for a full sync; `false` for incremental (cursor-based) sync.
    pub full: bool,
    /// Optional relative window (`now - since`) restricting fetched items.
    pub since: Option<Duration>,
}

/// Result of a [`Connector::sync`] call.
///
/// `new_cursor` is the opaque, per-connector sync progress token the
/// supervisor persists via `KnowledgeGraph::update_sync_cursor` so the next
/// incremental sync resumes from where this one left off. `fetched` is the
/// number of raw items staged for extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncOutcome {
    /// Number of raw items fetched and staged for extraction.
    pub fetched: u32,
    /// Updated sync cursor to persist, or `None` if unchanged.
    pub new_cursor: Option<String>,
    /// When the sync completed (server-side wall clock).
    pub fetched_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Transient result of probing a connector's service right now.
///
/// This is a **runtime probe**, distinct from the *persisted* lifecycle state
/// (`mimir_knowledge::models::enums::ConnectorStatus` /
/// `ConnectorAuthState`). The supervisor calls [`Connector::health`] and maps
/// the outcome onto the persisted columns — for example a probe of
/// [`HealthStatus::AuthExpired`](Self::AuthExpired) prompts the supervisor to
/// set `auth_state = Expired` and `status = Paused`. Variant names are
/// intentionally distinct from the persisted-enum variants to avoid confusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Service is reachable and credentials are valid.
    Online,
    /// Service is unreachable (network down, host unresponsive).
    Offline,
    /// Reachable but returning partial / repeated failures.
    Degraded,
    /// Auth token expired or revoked; re-authentication required.
    AuthExpired,
    /// Connector has not been configured / authenticated yet.
    NotConfigured,
}

// ---------------------------------------------------------------------------
// Write-back (optional)
// ---------------------------------------------------------------------------

/// A write-back action for an optional [`Connector::act`] implementation
/// (e.g. creating a calendar event). Backends that support write-back
/// interpret `kind` and `payload`; backends that do not leave the default
/// implementation, which returns [`ConnectorError::UnsupportedAction`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorAction {
    /// Backend-specific action tag (e.g. `"create_event"`, `"delete_event"`).
    pub kind: String,
    /// Action-specific payload as a JSON object.
    pub payload: serde_json::Value,
}

/// Outcome of a successful write-back action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    /// Whether the action completed successfully.
    pub success: bool,
    /// Native id of the created/modified item, if applicable.
    pub native_id: Option<String>,
    /// Optional human-readable detail.
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Runtime connector trait — the interface every service ingestion worker
/// implements.
///
/// Each trait object represents a single configured connector *instance* (one
/// row in the `connectors` table): [`Connector::id`] is the instance slug,
/// [`Connector::connector_type`] is the provenance/reliability axis, and
/// [`Connector::name`] is the display name. Credentials and shared services
/// (e.g. `Arc<dyn LlmBackend>` per decision D′) are injected at construction
/// by the factory (F7) / secret store (F10), so [`Connector::authenticate`]
/// takes no arguments.
///
/// # Mutable state — `&self`, not `&mut self`
///
/// Every method takes `&self` (matching the workspace `Tool` trait), so the
/// trait is callable through the shared `Arc<dyn Connector>` storage used by
/// the registry (F7) and supervisor (F8). A connector that needs to mutate
/// internal state (its raw-item buffer, sync cursor, cached auth state) owns
/// that state behind interior mutability (e.g. `tokio::sync::Mutex`) inside
/// its concrete type — the trait surface stays shared-reference friendly.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Stable, unique, slug-style identifier for this connector instance.
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// Provenance and reliability axis for this connector
    /// (`Gmail` / `Calendar` / `Photos` / …).
    fn connector_type(&self) -> ConnectorType;

    /// How the supervisor should run this connector (polling vs push).
    fn mode(&self) -> ConnectorMode;

    /// JSON Schema describing the connector's configuration surface.
    fn config_schema(&self) -> serde_json::Value;

    /// Perform (or refresh) authentication with the service.
    ///
    /// Credentials are injected at construction; this performs the handshake
    /// and returns the resulting auth state for the supervisor to persist via
    /// `KnowledgeGraph::set_auth_state`.
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError>;

    /// Probe the service's current reachability and auth health.
    ///
    /// Returns a transient [`HealthStatus`]; the supervisor maps it onto the
    /// persisted lifecycle columns. Must not perform a full data sync.
    async fn health(&self) -> Result<HealthStatus, ConnectorError>;

    /// Fetch raw items from the service into the connector's internal buffer.
    ///
    /// Does **not** extract facts or touch the knowledge graph. Returns sync
    /// stats and an updated cursor for the supervisor to persist.
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError>;

    /// Drain buffered raw items into typed, parsed normalized facts.
    ///
    /// Entity ids are **not** resolved here — that is `normalize_and_insert`'s
    /// job. The supervisor calls this after [`sync`](Self::sync) and then
    /// inserts the returned facts through the shared pipeline.
    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError>;

    /// Optional write-back to the service.
    ///
    /// Default implementation declines the action. Backends that support
    /// write-back (e.g. Calendar event creation in C4) override this.
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError> {
        Err(ConnectorError::UnsupportedAction(action.kind))
    }

    /// Remove all local data and credentials for this connector instance.
    ///
    /// The supervisor additionally cascades the deletion to knowledge-graph
    /// facts with this `connector_instance_id` via the existing trash
    /// machinery; this method handles the connector-local cleanup.
    async fn forget(&self) -> Result<(), ConnectorError>;
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Constructs a [`Connector`] instance from its persisted configuration.
///
/// The registry (F7 / issue #184) maps `(connector_type, backend)` to one
/// `ConnectorFactory`. When the supervisor (F8) or the `connector add` CLI
/// path needs to instantiate a configured connector, it looks up the factory
/// for the row's `(type, backend)` and calls [`ConnectorFactory::create`]
/// with the `config_json` parsed value.
///
/// # Construction context
///
/// For Phase 3 V1 `create` takes only the config payload, matching the issue
/// spec. Decision D′ of the Phase 3 plan states that connectors receive the
/// shared `Arc<dyn LlmBackend>` at construction, and F10 will inject
/// credentials via the `SecretStore`; those dependencies land with F8 / F10
/// and are not yet available. When they arrive the factory signature will be
/// extended to accept a construction context — a breaking change to an
/// internal API, which is explicitly acceptable per the project's breaking
/// changes policy.
///
/// # Object safety
///
/// The trait is `Send + Sync` with a single non-generic method returning
/// `Arc<dyn Connector>`, so it is object-safe and stored by the registry as
/// `Arc<dyn ConnectorFactory>`.
pub trait ConnectorFactory: Send + Sync {
    /// Build a ready-to-run connector instance from its configuration.
    ///
    /// `config` is the deserialised `config_json` column of the
    /// `connectors` row. Construction must be cheap and synchronous: network
    /// handshakes happen later via [`Connector::authenticate`], and data
    /// fetches via [`Connector::sync`].
    fn create(
        &self,
        config: serde_json::Value,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError>;
}
