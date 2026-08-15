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

use crate::secrets::SecretStore;
use mimir_core::geocoder::Geocoder;
use mimir_core::llm::LlmBackend;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

/// Shared services injected into a connector at construction (Phase 3 C2 /
/// issue #196).
///
/// Decision D′ of the Phase 3 plan anticipated extending the factory
/// signature to a construction context once connectors needed shared
/// dependencies beyond their `config_json`. The Photos connector (C2) is the
/// first: it reverse-geocodes EXIF GPS into a place name at extraction time,
/// so it needs the shared [`Geocoder`]. The Calendar and Email connectors
/// (C3 / C5) load credentials from the shared [`SecretStore`], the daemon
/// injects the canonical user identity name (A1), and the Email connector's
/// LLM extraction (C7) clones the shared `Arc<dyn LlmBackend>`.
///
/// The context is passed by shared reference to [`ConnectorFactory::create`];
/// factories clone out whatever `Arc` they need (`Option<Arc<_>>::clone` is
/// cheap), so no connector takes ownership of a shared service.
#[derive(Debug, Default, Clone)]
pub struct ConnectorContext {
    /// Pluggable geocoder shared across the Photos connector (C2), the
    /// entity-locations write path (S3), and the Location Search tool (#98).
    /// `None` when no geocoder is configured (the daemon initialises a
    /// Nominatim backend in `mimir-server`; tests inject a mock).
    pub geocoder: Option<std::sync::Arc<dyn Geocoder>>,

    /// Pluggable credential store shared across connectors that need secrets
    /// at construction/sync time (Calendar C3 / #197, Email C5). `None` when
    /// no store is configured (the daemon initialises a `FileSecretStore` in
    /// `mimir-server`; tests inject an `InMemorySecretStore`). Connectors
    /// load their [`SecretBundle`](crate::secrets::SecretBundle) by slug
    /// (the `__slug` injected by the supervisor).
    pub secret_store: Option<std::sync::Arc<dyn SecretStore>>,

    /// Canonical user identity name (the `config.toml` `[identity] name`),
    /// injected by the daemon (A1) so connectors author user-scoped facts
    /// against the same entity the daemon resolves as `user_entity_id`.
    /// `None` when no identity is configured; the Calendar connector (C4)
    /// then omits the primary `has_event` fact and emits only the
    /// location/attendee facts (so the event will not surface in the
    /// user's "Upcoming" memory section, which is scoped to the user
    /// entity).
    pub user_identity: Option<String>,

    /// Shared LLM backend for connector background work (C7 / #201).
    ///
    /// Connectors that need LLM extraction (the Email connector's prose
    /// layer) clone this `Arc<dyn LlmBackend>` at construction and call
    /// [`LlmBackend::system_chat_message`] so their calls route through
    /// the shared `LlmWorkerPool`'s **system queue** — below user-chat
    /// priority, so a one-call-at-a-time provider is never starved by a
    /// background extraction burst. `None` when no backend is configured
    /// (the daemon injects the shared `LlmClient`; tests inject a
    /// `MockLlmClient`). Connectors that need no LLM ignore it.
    pub llm_backend: Option<std::sync::Arc<dyn LlmBackend>>,
}

/// Normalise a canonical user identity name for storage on a connector or
/// context: surrounding whitespace is trimmed and an empty/whitespace-only
/// name becomes `None`, so a misconfigured `[identity]` never authors facts
/// against an empty-string or padded `Person` entity. Shared by
/// [`ConnectorContext::with_user_identity`] and the Calendar, Email, and
/// Photos constructors (DRY).
pub(crate) fn normalize_user_identity(name: Option<String>) -> Option<String> {
    name.filter(|n| !n.trim().is_empty())
        .map(|n| n.trim().to_string())
}

impl ConnectorContext {
    /// Build a context carrying the supplied geocoder and no secret store.
    pub fn new(geocoder: Option<std::sync::Arc<dyn Geocoder>>) -> Self {
        Self {
            geocoder,
            secret_store: None,
            user_identity: None,
            llm_backend: None,
        }
    }

    /// An empty context with no shared services (used by the registry's
    /// config-only [`ConnectorRegistry::create`] shortcut and by connectors
    /// that need no injected dependencies).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Attach a shared [`SecretStore`] to this context (builder).
    ///
    /// Connectors that need credentials (Calendar C3 / #197, Email C5) clone
    /// the `Arc<dyn SecretStore>` out of the context and load their bundle by
    /// slug. Connectors that need no secrets ignore this. Consumes and returns
    /// `self` so it chains with [`with_geocoder`](Self::with_geocoder).
    pub fn with_secret_store(mut self, store: std::sync::Arc<dyn SecretStore>) -> Self {
        self.secret_store = Some(store);
        self
    }

    /// Attach a shared [`Geocoder`] to this context (builder), mirroring
    /// [`ConnectorSupervisor::with_geocoder`].
    pub fn with_geocoder(mut self, geocoder: std::sync::Arc<dyn Geocoder>) -> Self {
        self.geocoder = Some(geocoder);
        self
    }

    /// Attach the canonical user identity name to this context (builder).
    ///
    /// The daemon populates this from `config.toml`'s `[identity] name` (the
    /// same value it resolves to `user_entity_id`); connectors that author
    /// user-scoped facts read it at construction. An empty/whitespace name is
    /// treated as "no identity" so a misconfigured `[identity]` does not emit
    /// facts authored by an empty-string entity.
    pub fn with_user_identity(mut self, name: impl Into<String>) -> Self {
        self.user_identity = normalize_user_identity(Some(name.into()));
        self
    }

    /// Attach a shared [`LlmBackend`] to this context (builder), mirroring
    /// [`ConnectorSupervisor::with_llm_backend`]. Connectors that perform LLM
    /// extraction (Email C7 / #201) clone the `Arc<dyn LlmBackend>` out of
    /// the context and route calls through [`LlmBackend::system_chat_message`].
    pub fn with_llm_backend(mut self, backend: std::sync::Arc<dyn LlmBackend>) -> Self {
        self.llm_backend = Some(backend);
        self
    }
}

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

/// Map an LLM-backend failure onto a connector error so the supervisor's
/// retry loop can surface it. Network-level failures keep their category;
/// every other provider/parse/queue failure becomes a generic connector
/// failure (retryable, but not network-specific).
impl From<mimir_core::llm::LlmError> for ConnectorError {
    fn from(error: mimir_core::llm::LlmError) -> Self {
        match error {
            mimir_core::llm::LlmError::Network(err) => ConnectorError::Network(err.to_string()),
            other => ConnectorError::Other(other.to_string()),
        }
    }
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
///
/// [`Default`] is an incremental sync with no time-window hint — the same
/// options the supervisor uses for an automatic polling cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
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
    /// stats and an updated cursor for the supervisor to persist. The
    /// connector must **not** adopt the returned cursor into its own
    /// in-memory state here — the supervisor persists it first and hands it
    /// back via [`on_cycle_succeeded`](Self::on_cycle_succeeded) once the
    /// cycle fully succeeded, so a failed cycle re-syncs from the last
    /// confirmed cursor (issue #314).
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError>;

    /// Called by the supervisor after a cycle fully succeeded — `sync`
    /// fetched, `extract` drained, deletions trashed, facts inserted, and
    /// the returned cursor persisted via
    /// `KnowledgeGraph::update_sync_progress_and_durable_state` — so the
    /// connector may adopt the persisted cursor as its in-memory progress
    /// marker for the next incremental sync.
    ///
    /// Failure-safe incremental sync (issue #314): the persisted
    /// `connectors.sync_cursor` advances only on a fully successful cycle, so
    /// an in-memory cursor adopted earlier (inside [`sync`](Self::sync))
    /// would skip the failed cycle's window on the next in-process cycle —
    /// only a daemon restart, which re-seeds from the persisted cursor,
    /// would recover it. `new_cursor` is the cursor the supervisor just
    /// persisted (`None` means unchanged). Connectors whose progress lives
    /// solely in the persisted column — or that re-deliver failed windows by
    /// other means (e.g. a durable retry ledger, issue #262) — leave the
    /// default no-op.
    async fn on_cycle_succeeded(&self, _new_cursor: Option<&str>) {}

    /// Drain buffered raw items into typed, parsed normalized facts.
    ///
    /// Entity ids are **not** resolved here — that is `normalize_and_insert`'s
    /// job. The supervisor calls this after [`sync`](Self::sync) and then
    /// inserts the returned facts through the shared pipeline.
    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError>;

    /// Opaque connector-side durable state to persist after a successful
    /// extraction cycle (issue #262).
    ///
    /// The supervisor persists the returned value via
    /// `KnowledgeGraph::update_sync_progress_and_durable_state` — committed
    /// in the same transaction as the sync cursor, so a crash between the
    /// two writes cannot advance the cursor without its durable state (PR
    /// #318 review) — and re-injects it at construction (as the
    /// `__durable_state` config key), so connector-owned state that must
    /// survive a daemon restart — the Email connector's bounded
    /// LLM-extraction retry ledger — lives outside the in-memory raw-item
    /// buffer. `None` means "nothing changed since the last persist";
    /// connectors that keep no durable state leave the default. The
    /// returned value is not consumed: after the supervisor's combined
    /// database commit succeeds it calls
    /// [`durable_state_persisted`](Connector::durable_state_persisted), so a
    /// failed write never loses state.
    fn durable_state(&self) -> Option<String> {
        None
    }

    /// Acknowledge that the last [`durable_state`](Connector::durable_state)
    /// value was persisted by the supervisor (called only after the
    /// `update_durable_state` database write succeeded). Connectors use this
    /// to mark their state clean; a default no-op for connectors that keep
    /// no durable state.
    fn durable_state_persisted(&self) {}

    /// Report the buffered server-side removals (tombstones) as the set of
    /// `raw_reference`s whose knowledge-graph facts should be trashed.
    ///
    /// Called by the supervisor after [`sync`](Self::sync) and
    /// [`extract`](Self::extract) on every cycle. Each returned string is
    /// matched against this instance's `sources.raw_reference` rows and the
    /// matching facts are trashed through the shared trash machinery
    /// (recoverable for 30 days, inferred children evaluated). Idempotent:
    /// a removal reported twice trashes nothing the second time.
    ///
    /// The report is **non-destructive**: the pending tombstones stay
    /// buffered until the supervisor calls [`acknowledge_deletions`] after
    /// the cycle's trashing, fact insertion, and cursor persistence all
    /// succeeded, so a transient failure re-reports the same removals on the
    /// next cycle instead of losing them. Connectors whose services cannot
    /// report deletions keep the default empty set.
    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        Ok(Vec::new())
    }

    /// Drop the acknowledged server-side removals from the connector's
    /// pending tombstone buffer.
    ///
    /// Called by the supervisor only after the cycle's deletion trashing,
    /// fact insertion, and cursor persistence all succeeded, with exactly the
    /// `raw_reference`s [`extract_deletions`](Self::extract_deletions)
    /// returned this cycle. Connectors that buffer deletions must remove the
    /// acknowledged references so a fully processed removal is not
    /// re-reported forever; the default implementation has no buffer.
    async fn acknowledge_deletions(&self, _deleted: &[String]) -> Result<(), ConnectorError> {
        Ok(())
    }

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
/// `ConnectorFactory`. When the supervisor (F8) needs to instantiate a
/// configured connector, it looks up the factory for the row's `(type,
/// backend)` and calls [`ConnectorFactory::create`] with the `config_json`
/// parsed value and the shared-services [`ConnectorContext`].
///
/// # Construction context
///
/// `create` takes the config payload plus a [`ConnectorContext`] carrying the
/// shared services a backend may need at construction: the [`Geocoder`]
/// (Photos reverse geocoding, C2 / #196), the [`SecretStore`] (Calendar /
/// Email credentials, F10 / #187), the canonical user identity name (A1), and
/// the shared `Arc<dyn LlmBackend>` (Email LLM extraction, C7 / #201, routed
/// through the shared pool's system queue per decision D′). Backends that
/// need no shared services ignore the context.
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
    /// fetches via [`Connector::sync`]. `ctx` carries shared services the
    /// backend may need at construction (e.g. the [`Geocoder`] for the Photos
    /// connector, Phase 3 C2 / #196); backends that need none ignore it.
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError>;
}

// ---------------------------------------------------------------------------
// Tests (ConnectorContext secret-store wiring, Phase 3 C3 / #197)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{InMemorySecretStore, SecretBundle};
    use std::sync::Arc;

    #[test]
    fn context_with_secret_store_carries_store() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
        let ctx = ConnectorContext::empty().with_secret_store(Arc::clone(&store));
        assert!(ctx.secret_store.is_some(), "secret_store should be set");
        assert!(ctx.geocoder.is_none(), "geocoder untouched");
        // The stored Arc is the same instance.
        assert!(Arc::ptr_eq(ctx.secret_store.as_ref().unwrap(), &store));
    }

    #[test]
    fn context_empty_has_no_secret_store() {
        let ctx = ConnectorContext::empty();
        assert!(ctx.secret_store.is_none());
    }

    #[tokio::test]
    async fn context_secret_store_is_usable() {
        let store = Arc::new(InMemorySecretStore::new());
        let ctx =
            ConnectorContext::empty().with_secret_store(store.clone() as Arc<dyn SecretStore>);
        store
            .store(
                "calendar-personal",
                &SecretBundle::AppPassword {
                    password: "hunter2".into(),
                },
            )
            .await
            .unwrap();
        let loaded = ctx
            .secret_store
            .as_ref()
            .unwrap()
            .load("calendar-personal")
            .await
            .unwrap();
        assert_eq!(
            loaded,
            Some(SecretBundle::AppPassword {
                password: "hunter2".into()
            })
        );
    }

    #[test]
    fn context_with_user_identity_carries_name() {
        let ctx = ConnectorContext::empty().with_user_identity("Devansh");
        assert_eq!(ctx.user_identity.as_deref(), Some("Devansh"));
    }

    #[test]
    fn context_with_user_identity_ignores_blank() {
        let ctx = ConnectorContext::empty().with_user_identity("   ");
        assert!(ctx.user_identity.is_none(), "blank identity is no identity");
        let ctx = ConnectorContext::empty().with_user_identity("");
        assert!(ctx.user_identity.is_none());
    }

    #[test]
    fn context_with_user_identity_trims_surrounding_whitespace() {
        // A padded `[identity] name` is stored trimmed, not verbatim, so it
        // resolves to the same entity instead of a duplicate person (#248).
        let ctx = ConnectorContext::empty().with_user_identity("  Devansh  ");
        assert_eq!(ctx.user_identity.as_deref(), Some("Devansh"));
    }

    #[test]
    fn context_empty_has_no_user_identity() {
        assert!(ConnectorContext::empty().user_identity.is_none());
    }
}
