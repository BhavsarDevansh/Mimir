//! `ConnectorSupervisor` — supervised per-connector task lifecycle
//! (Phase 3 F8 / issue #185).
//!
//! One supervised tokio task per *active* connector instance. Centralises
//! spawn, restart-with-backoff, circuit breaker, startup restore, graceful
//! shutdown, and cursor persistence. All status / auth / cursor writes go
//! through the [`mimir_knowledge::KnowledgeGraph`] facade — this crate never
//! holds a `sqlx` pool.
//!
//! # Ingestion model
//!
//! The supervisor owns the database half of the two-step ingestion model
//! (see [`crate::connector`]). Per cycle it calls
//! [`Connector::health`] → [`Connector::sync`] → [`Connector::extract`] and
//! then funnels the resulting facts through
//! [`mimir_knowledge::normalize::normalize_and_insert`] with a
//! [`Provenance::connector`] provenance. The cursor returned by `sync` is
//! persisted via [`KnowledgeGraph::update_sync_cursor`] so `mimir stop`
//! mid-sync does not re-fetch already-ingested items.
//!
//! # Lifecycle mapping
//!
//! - **Success:** reset failure count, persist cursor, clear `last_error`,
//!   keep `status = Active`.
//! - **Sync/extract error:** increment failures, write `last_error` (status
//!   stays `Active`), exponential backoff; after `max_failures` consecutive
//!   failures → `status = Error`, stop auto-restart (manual `resume` required).
//! - **Task panic:** counted as a failure (via `JoinError::is_panic`), same
//!   backoff + breaker path.
//! - **Auth expired** (`HealthStatus::AuthExpired`): `auth_state = Expired`,
//!   `status = Paused`, task exits (not auto-restarted).
//! - **Shutdown:** observe the shared `watch::Receiver<bool>`; abort in-flight
//!   cycles and exit. The cursor reflects the last *completed* sync.
//!
//! `yield-on-user-activity` is deferred for V1.
//!
//! # Instance identity injection
//!
//! [`ConnectorSupervisor::restore`] augments each row's `config_json` with
//! `__slug`, `__ctype`, and `__instance_id` before handing it to the
//! [`crate::ConnectorFactory`]. Because the V1 factory signature is
//! `create(config)` only (decision #2: the LLM/SecretStore construction context
//! is deferred to F10 / the first real backend), this is how a connector
//! instance learns which `connectors` row it represents. Real backends read
//! these keys to recover their identity without an extra factory argument.
//!
//! # Manual sync triggering (F9 / #186)
//!
//! [`ConnectorSupervisor::trigger_sync`] wakes a connector's runner from its
//! polling-interval wait so a sync runs immediately with caller-supplied
//! [`SyncOptions`] (e.g. `--full`). A one-permit `Semaphore` per connector
//! serialises concurrent callers — overlapping triggers queue rather than
//! launching parallel cycles — and a per-connector request channel carries
//! the options and returns the cycle's [`TriggerOutcome`]. Push-mode
//! connectors have no polling interval to preempt, so manual triggers are
//! rejected for them in V1.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{info, warn};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::Connector as ConnectorRow;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::{Provenance, normalize_and_insert};

use crate::connector::ActionResult;
use crate::connector::ConnectorContext;
use crate::connector::ConnectorMode;
use crate::connector::{
    Connector, ConnectorAction, ConnectorError, HealthStatus, SyncOptions, SyncOutcome,
};
use crate::registry::ConnectorRegistry;
use crate::secrets::SecretStore;
use mimir_core::geocoder::Geocoder;
use mimir_core::llm::LlmBackend;

/// Tunable parameters for a [`ConnectorSupervisor`].
///
/// Injected at construction (no environment mutation, per the project safety
/// policy). Sensible defaults suit a single-user daemon; tests override with
/// millisecond values for fast, deterministic runs.
///
/// Exponential backoff here is *deterministic*: `base_backoff * 2^(n-1)`,
/// capped at `max_backoff`. Randomised jitter / rate-limit primitives belong
/// to F12 (issue #189) and are intentionally not re-implemented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorConfig {
    /// Consecutive failures before the circuit breaker trips and the
    /// connector is moved to `Error` (requires manual `resume`).
    pub max_failures: u32,
    /// Initial exponential-backoff delay applied after the first failure.
    pub base_backoff: Duration,
    /// Cap on the exponentially-growing backoff delay.
    pub max_backoff: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            max_failures: 5,
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
        }
    }
}

/// Errors raised by supervisor operations.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error(transparent)]
    Knowledge(#[from] mimir_knowledge::KnowledgeError),
    #[error(transparent)]
    Connector(#[from] crate::ConnectorError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The connector row exists but its `connector_type_id` does not map to a
    /// known [`ConnectorType`] (Phase 3 A2 / #203).
    #[error("connector {id} has an unknown connector_type id {type_id}")]
    UnknownConnectorType { id: i32, type_id: i16 },
}

/// Errors raised by [`ConnectorSupervisor::act`] (Phase 3 A2 / #203).
///
/// Infrastructure failures of the dispatch mechanism: an unknown instance, an
/// unresolvable connector type, a knowledge-graph lookup failure, or the
/// connector's own [`ConnectorError`] (e.g. `UnsupportedAction`).
#[derive(Debug, thiserror::Error)]
pub enum ActError {
    #[error(transparent)]
    Knowledge(#[from] mimir_knowledge::KnowledgeError),
    /// No connector row exists with the given instance id.
    #[error("no connector with id {0}")]
    NotFound(i32),
    /// The connector row exists but its `connector_type_id` does not map to a
    /// known [`ConnectorType`].
    #[error("connector {id} has an unknown connector_type id {type_id}")]
    UnknownType { id: i32, type_id: i16 },
    #[error(transparent)]
    Connector(#[from] crate::ConnectorError),
}

// ---------------------------------------------------------------------------
// Manual sync trigger types (F9 / #186)
// ---------------------------------------------------------------------------

/// A manual sync request queued from [`ConnectorSupervisor::trigger_sync`] to
/// a connector's runner task. Carries the caller's [`SyncOptions`] and a
/// [`oneshot::Sender`] to deliver the cycle's outcome back to the caller.
struct TriggerRequest {
    options: SyncOptions,
    reply: oneshot::Sender<TriggerOutcome>,
}

/// Capacity of the per-connector trigger channel.
///
/// The per-connector [`Semaphore`] (one permit) is held across the send and
/// the reply await, so at most one trigger request is ever in flight per
/// connector — a previous request is always drained and replied before the
/// next caller is allowed to send. A capacity of one therefore never blocks
/// the sender and is the smallest sufficient buffer.
const TRIGGER_CHANNEL_CAPACITY: usize = 1;

/// Outcome of a manually-triggered sync cycle, returned to the caller of
/// [`ConnectorSupervisor::trigger_sync`].
///
/// Mirrors the runner's internal [`CycleOutcome`]: a successful cycle reports
/// the connector's [`SyncOutcome`] stats; `AuthExpired` reports that the
/// service rejected the connector's credentials (the supervisor has already
/// paused it); `Failed` reports a recoverable cycle error (panic, offline,
/// parse failure, …). Infrastructure problems (unknown id, not running,
/// push-mode, runner dropped mid-sync) surface as [`TriggerError`] instead.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerOutcome {
    /// The cycle succeeded.
    Ok {
        /// Number of raw items fetched and staged for extraction.
        fetched: u32,
        /// Updated sync cursor the supervisor persisted, or `None` if unchanged.
        new_cursor: Option<String>,
    },
    /// The service reported expired auth; the connector has been paused.
    AuthExpired,
    /// The cycle failed with a recoverable error.
    Failed(String),
}

/// Errors raised by [`ConnectorSupervisor::trigger_sync`] /
/// [`ConnectorSupervisor::trigger_sync_by_slug`].
///
/// These are *infrastructure* failures of the trigger mechanism itself — the
/// cycle's own success/failure is reported via [`TriggerOutcome`].
#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    /// A knowledge-graph lookup failed while resolving the connector.
    #[error(transparent)]
    Knowledge(#[from] mimir_knowledge::KnowledgeError),
    /// No connector row exists with the given instance id.
    #[error("no connector with id {0}")]
    NotFound(i32),
    /// No connector row exists with the given slug.
    #[error("no connector with slug `{0}`")]
    NotFoundSlug(String),
    /// The connector exists but is not running (it is `Paused`, `Error`, or
    /// `Setup`, or its runner has exited). Resume it before triggering a sync.
    #[error("connector {id} is not running (status: {status:?})")]
    NotRunning {
        /// Connector instance id.
        id: i32,
        /// Persisted lifecycle status, if the row could be loaded.
        status: Option<ConnectorStatus>,
    },
    /// The connector runs in push mode. Manual sync triggers preempt the
    /// polling interval, which push-mode connectors do not have; push-mode
    /// manual sync is deferred to a later Phase 3 issue.
    #[error(
        "connector {id} runs in push mode; manual sync trigger is not supported for push connectors"
    )]
    PushUnsupported {
        /// Connector instance id.
        id: i32,
    },
    /// The runner task stopped (shutdown / breaker / auth-expiry) while the
    /// triggered cycle was in flight, before it could report an outcome.
    #[error("connector {0} runner stopped before the sync completed")]
    RunnerDropped(i32),
}

/// Per-connector bookkeeping held by the supervisor alongside the runner task.
///
/// `trigger_tx` and `semaphore` implement manual sync triggering
/// (F9 / #186): [`ConnectorSupervisor::trigger_sync`] acquires the one-permit
/// `semaphore` (serialising concurrent callers) and sends a [`TriggerRequest`]
/// down `trigger_tx`; the runner drains it in its post-cycle wait and runs a
/// cycle with the request's [`SyncOptions`].
struct ConnectorHandle {
    /// The supervised runner task.
    task: JoinHandle<()>,
    /// The live connector instance (Phase 3 A2 / #203). Kept so
    /// [`ConnectorSupervisor::act`] can dispatch write-back actions to the
    /// running, authenticated instance without re-instantiating it. Cloned
    /// from the same `Arc<dyn Connector>` moved into the runner task, so both
    /// share one underlying instance.
    connector: Arc<dyn Connector>,
    /// Connector mode, captured at spawn so `trigger_sync` can reject push
    /// connectors without holding the connector instance.
    mode: ConnectorMode,
    /// Sender half of the per-connector trigger channel.
    trigger_tx: mpsc::Sender<TriggerRequest>,
    /// One-permit semaphore serialising concurrent `trigger_sync` callers.
    semaphore: Arc<Semaphore>,
}

/// Owns one supervised task per active connector instance.
///
/// Constructed after the knowledge graph and (eventually) LLM pool are up.
/// [`ConnectorSupervisor::restore`] loads the `connectors` table and spawns a
/// runner task for every row whose status is [`ConnectorStatus::Active`];
/// `Paused` / `Error` / `Setup` rows are left down. The shared shutdown
/// `watch::Receiver<bool>` is cloned into every runner so a single
/// `mimir stop` (or OS signal) drains them all.
pub struct ConnectorSupervisor {
    registry: Arc<ConnectorRegistry>,
    kg: Arc<KnowledgeGraph>,
    config: SupervisorConfig,
    shutdown: watch::Receiver<bool>,
    /// Shared services injected into every connector at construction (Phase 3
    /// C2 / #196 for the geocoder, C3 / #197 for the secret store). Built from
    /// [`with_geocoder`](Self::with_geocoder) and
    /// [`with_secret_store`](Self::with_secret_store); empty by default so
    /// connectors that need no injected services are unaffected.
    context: ConnectorContext,
    handles: Mutex<HashMap<i32, ConnectorHandle>>,
}

impl std::fmt::Debug for ConnectorSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `ConnectorHandle` and the `watch::Receiver` are not `Debug`, and
        // the handles map is private, so report the registry + a running-count
        // proxy rather than recursing. This mirrors the `ConnectorRegistry`
        // Debug impl and keeps `AppState`'s `#[derive(Debug)]` working.
        f.debug_struct("ConnectorSupervisor")
            .field("registry", &self.registry)
            .finish_non_exhaustive()
    }
}

impl ConnectorSupervisor {
    /// Create a supervisor over a registry, knowledge graph, and the shared
    /// shutdown signal.
    pub fn new(
        registry: Arc<ConnectorRegistry>,
        kg: Arc<KnowledgeGraph>,
        config: SupervisorConfig,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            registry,
            kg,
            config,
            shutdown,
            context: ConnectorContext::empty(),
            handles: Mutex::new(HashMap::new()),
        }
    }

    /// Inject a shared geocoder made available to every connector this
    /// supervisor constructs (Phase 3 C2 / #196).
    ///
    /// The Photos connector reverse-geocodes EXIF GPS into a place name at
    /// extraction time; the geocoder is cloned out of the context by the
    /// factory at construction. Must be called before [`restore`] so already
    /// spawned runners receive it. Other connectors ignore the geocoder.
    ///
    /// [`restore`]: Self::restore
    pub fn with_geocoder(mut self, geocoder: Arc<dyn Geocoder>) -> Self {
        self.context.geocoder = Some(geocoder);
        self
    }

    /// Inject a shared [`SecretStore`] made available to every connector this
    /// supervisor constructs (Phase 3 C3 / #197).
    ///
    /// Connectors that need credentials (Calendar, Email) clone the
    /// `Arc<dyn SecretStore>` out of the context at construction and load
    /// their [`SecretBundle`](crate::secrets::SecretBundle) by slug (the
    /// `__slug` injected into `config_json` by [`instantiate`](Self::instantiate)).
    /// Must be called before [`restore`] so already-spawned runners receive
    /// it. Connectors that need no secrets ignore it.
    ///
    /// [`restore`]: Self::restore
    pub fn with_secret_store(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.context.secret_store = Some(store);
        self
    }

    /// The shared [`SecretStore`] injected via
    /// [`with_secret_store`](Self::with_secret_store), if any.
    ///
    /// Exposed so the daemon's connector removal route can delete the
    /// credential entry keyed by a connector's slug when the instance is
    /// deleted, preventing a later same-slug connector from loading the
    /// deleted instance's stored credentials. Returns `None` when no store
    /// is configured (the daemon start path is best-effort; tests and
    /// sandboxed runs may have no `FileSecretStore`), in which case there is
    /// nothing to clean up.
    pub fn secret_store(&self) -> Option<Arc<dyn SecretStore>> {
        self.context.secret_store.clone()
    }

    /// Inject the canonical user identity name (the `config.toml`
    /// `[identity] name`) into the shared [`ConnectorContext`] so connectors
    /// that author user-scoped facts — the Calendar connector (C4 / #198),
    /// which emits `user has_event <event>` so the event surfaces in the
    /// user's "Upcoming" memory section — share one source of truth with the
    /// daemon's `user_entity_id` resolution. An empty/whitespace name is
    /// ignored (treated as "no identity"). Must be called before [`restore`].
    pub fn with_user_identity(mut self, name: impl Into<String>) -> Self {
        let name = name.into().trim().to_string();
        if !name.is_empty() {
            self.context.user_identity = Some(name);
        }
        self
    }

    /// Inject the shared [`LlmBackend`] made available to every connector
    /// this supervisor constructs (Phase 3 C7 / #201).
    ///
    /// The Email connector's prose-extraction layer routes its LLM calls
    /// through [`LlmBackend::system_chat_message`] so they sit on the
    /// shared `LlmWorkerPool`'s system queue (below user-chat priority): a
    /// one-call-at-a-time provider is never starved by a background
    /// extraction burst, and a queued user chat preempts a waiting connector
    /// call. Must be called before [`restore`] so already-spawned runners
    /// receive it. Connectors that need no LLM ignore it.
    ///
    /// [`restore`]: Self::restore
    pub fn with_llm_backend(mut self, backend: Arc<dyn LlmBackend>) -> Self {
        self.context.llm_backend = Some(backend);
        self
    }

    /// Spawn a runner task for every `Active` connector row.
    ///
    /// `Paused` / `Error` / `Setup` rows are not auto-spawned. Rows whose
    /// `(type, backend)` has no registered factory, or whose `config_json` is
    /// invalid, are logged and skipped rather than aborting the whole
    /// restore. Returns the number of runner tasks spawned.
    pub async fn restore(&self) -> Result<usize, SupervisorError> {
        let rows = self.kg.list_connectors().await?;
        let mut spawned = 0usize;
        for row in rows {
            if row.status() != Some(ConnectorStatus::Active) {
                info!(connector_id = row.id, slug = %row.slug, status = ?row.status(), "skipping non-active connector at restore");
                continue;
            }
            let Some(connector_type) = row.connector_type() else {
                warn!(connector_id = row.id, slug = %row.slug, "unknown connector_type id {}; skipping", row.connector_type_id);
                continue;
            };
            let connector = match self.instantiate(&row, connector_type) {
                Ok(c) => c,
                Err(error) => {
                    warn!(connector_id = row.id, slug = %row.slug, error = %error, "failed to instantiate connector; skipping");
                    continue;
                }
            };
            self.spawn_into(row, connector_type, connector).await;
            spawned += 1;
        }
        Ok(spawned)
    }

    /// Instantiate, spawn a runner for, and register a single connector row
    /// (Phase 3 A2 / #203).
    ///
    /// Shared by [`restore`](Self::restore) (startup batch) and
    /// [`start`](Self::start) (single-instance resume / re-spawn). Stops any
    /// existing runner for `row.id` first so a re-spawn never leaves two
    /// handles for one instance. The connector is cloned: one `Arc` moves
    /// into the runner task, one is retained in the
    /// [`ConnectorHandle`] for [`act`](Self::act) dispatch.
    async fn spawn_into(
        &self,
        row: ConnectorRow,
        connector_type: ConnectorType,
        connector: Arc<dyn Connector>,
    ) {
        // Capture the mode up front so `trigger_sync` can reject push
        // connectors without holding the connector instance.
        let mode = connector.mode();
        let (trigger_tx, trigger_rx) = mpsc::channel(TRIGGER_CHANNEL_CAPACITY);
        let semaphore = Arc::new(Semaphore::new(1));
        let handle = tokio::spawn(run_connector(
            // One clone feeds the runner; the other is retained below.
            Arc::clone(&connector),
            self.kg.clone(),
            self.config,
            self.shutdown.clone(),
            row.id,
            connector_type,
            trigger_rx,
        ));
        self.handles.lock().await.insert(
            row.id,
            ConnectorHandle {
                task: handle,
                connector,
                mode,
                trigger_tx,
                semaphore,
            },
        );
        info!(connector_id = row.id, slug = %row.slug, backend = %row.backend, "spawned connector runner");
    }

    /// Trigger an immediate sync of the connector with the given instance id,
    /// bypassing its polling interval (Phase 3 F9 / #186).
    ///
    /// The caller's [`SyncOptions`] are delivered to the connector's runner,
    /// which runs a single cycle with them (`full` forces a non-incremental
    /// pass; `since` is a relative time-window hint). A one-permit semaphore
    /// per connector serialises concurrent callers — overlapping triggers
    /// queue rather than launching parallel cycles — and the method awaits
    /// the triggered cycle and returns its [`TriggerOutcome`].
    ///
    /// # Errors
    ///
    /// - [`TriggerError::NotFound`] — no connector row with `id`.
    /// - [`TriggerError::NotRunning`] — the connector is `Paused` / `Error` /
    ///   `Setup`, or its runner has exited (resume it first).
    /// - [`TriggerError::PushUnsupported`] — push-mode connectors have no
    ///   polling interval to preempt; push manual sync is deferred.
    /// - [`TriggerError::RunnerDropped`] — the runner stopped mid-sync
    ///   (shutdown / breaker / auth-expiry) before reporting an outcome.
    pub async fn trigger_sync(
        &self,
        id: i32,
        options: SyncOptions,
    ) -> Result<TriggerOutcome, TriggerError> {
        // Clone the sendable parts out of the lock before awaiting so the
        // mutex is never held across an await.
        let (trigger_tx, semaphore, mode, finished) = {
            let guard = self.handles.lock().await;
            match guard.get(&id) {
                Some(handle) => (
                    handle.trigger_tx.clone(),
                    handle.semaphore.clone(),
                    handle.mode,
                    handle.task.is_finished(),
                ),
                None => {
                    drop(guard);
                    let row = self.kg.get_connector(id).await?;
                    let Some(row) = row else {
                        return Err(TriggerError::NotFound(id));
                    };
                    return Err(TriggerError::NotRunning {
                        id,
                        status: row.status(),
                    });
                }
            }
        };
        if finished {
            // The runner exited (breaker / auth-expiry / shutdown). Reflect
            // the persisted status so the caller knows to resume.
            let row = self.kg.get_connector(id).await?;
            let status = row.and_then(|r| r.status());
            return Err(TriggerError::NotRunning { id, status });
        }
        if mode == ConnectorMode::Push {
            return Err(TriggerError::PushUnsupported { id });
        }
        // Serialise concurrent triggers: only one caller holds the permit at a
        // time, so overlapping `trigger_sync` calls queue rather than
        // launching parallel cycles. `acquire` errors only if the semaphore is
        // closed, which never happens for an `Arc<Semaphore>` we own.
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| TriggerError::RunnerDropped(id))?;
        let (reply_tx, reply_rx) = oneshot::channel();
        trigger_tx
            .send(TriggerRequest {
                options,
                reply: reply_tx,
            })
            .await
            .map_err(|_| TriggerError::RunnerDropped(id))?;
        // The runner replies once the cycle completes; a `RecvError` means it
        // exited (shutdown / breaker / auth-expiry) mid-cycle.
        reply_rx.await.map_err(|_| TriggerError::RunnerDropped(id))
    }

    /// Trigger an immediate sync by connector slug — convenience wrapper
    /// around [`trigger_sync`](Self::trigger_sync) that resolves the slug to
    /// an instance id via the knowledge graph.
    pub async fn trigger_sync_by_slug(
        &self,
        slug: &str,
        options: SyncOptions,
    ) -> Result<TriggerOutcome, TriggerError> {
        let row = self.kg.get_connector_by_slug(slug).await?;
        let Some(row) = row else {
            return Err(TriggerError::NotFoundSlug(slug.to_string()));
        };
        self.trigger_sync(row.id, options).await
    }

    /// Gracefully stop every runner: abort in-flight cycles and await exit.
    ///
    /// The shared shutdown `watch` is normally signalled first (so runners exit
    /// on their own); `abort` is a defensive fallback for stragglers.
    pub async fn shutdown(&self) {
        let handles: Vec<ConnectorHandle> =
            self.handles.lock().await.drain().map(|(_, h)| h).collect();
        for handle in &handles {
            handle.task.abort();
        }
        for handle in handles {
            let _ = handle.task.await;
        }
    }

    /// Number of runner tasks that are still alive (not yet finished).
    pub async fn running_count(&self) -> usize {
        self.handles
            .lock()
            .await
            .values()
            .filter(|handle| !handle.task.is_finished())
            .count()
    }

    /// Whether the runner for `id` is still alive.
    pub async fn is_running(&self, id: i32) -> bool {
        self.handles
            .lock()
            .await
            .get(&id)
            .is_some_and(|handle| !handle.task.is_finished())
    }

    /// Stop a single connector's runner task and remove it from the
    /// supervisor (issue #202 / Phase 3 A1).
    ///
    /// Aborts the runner in flight (cancelling any pending cycle), awaits its
    /// termination, and drops the [`ConnectorHandle`] so a subsequent
    /// [`restore`](Self::restore) or [`trigger_sync`](Self::trigger_sync)
    /// will treat the instance as down. The connector row is **not** deleted
    /// here — row lifecycle is the daemon's responsibility; this only manages
    /// the in-memory task. Persisting the current sync cursor happens in the
    /// runner's normal shutdown path; an aborted mid-cycle cycle is treated
    /// the same as `mimir stop` (the cursor reflects the last *completed*
    /// sync).
    ///
    /// Returns `true` if a runner was stopped, `false` if no live runner exists
    /// for `id` (already finished, never spawned, or previously stopped).
    pub async fn stop(&self, id: i32) -> bool {
        let handle = self.handles.lock().await.remove(&id);
        match handle {
            // Live runner: abort the in-flight cycle, await its termination,
            // and report that a runner was stopped.
            Some(handle) if !handle.task.is_finished() => {
                handle.task.abort();
                let _ = handle.task.await;
                true
            }
            // A stale handle whose task already completed naturally (e.g. an
            // unauthenticated connector whose runner exited at the auth
            // handshake) is cleaned up but reports no live runner was stopped.
            Some(_) => false,
            None => false,
        }
    }

    /// (Re)spawn a single connector's runner by instance id
    /// (Phase 3 A2 / #203).
    ///
    /// Loads the row, instantiates the connector via the registry, transitions
    /// it to [`ConnectorStatus::Active`] (clearing `last_error`), and spawns a
    /// supervised runner. Any existing runner for `id` is stopped first so a
    /// re-spawn never leaves two handles for one instance. Used by
    /// [`resume`](Self::resume) and (future) reconfig-on-restart.
    ///
    /// # Errors
    ///
    /// - [`SupervisorError::Knowledge`] — the row lookup or status write
    ///   failed, or no row matches `id` ([`KnowledgeError::ConnectorNotFound`]).
    /// - [`SupervisorError::UnknownConnectorType`] — the row's
    ///   `connector_type_id` is not a known [`ConnectorType`].
    /// - [`SupervisorError::Connector`] / [`SupervisorError::Json`] — the
    ///   row's `config_json` could not be parsed or the factory rejected it.
    pub async fn start(&self, id: i32) -> Result<(), SupervisorError> {
        // Stop any existing runner first so a re-spawn is idempotent.
        self.stop(id).await;

        let row = self
            .kg
            .get_connector(id)
            .await?
            .ok_or(SupervisorError::Knowledge(
                mimir_knowledge::KnowledgeError::ConnectorNotFound(id),
            ))?;
        let connector_type = row
            .connector_type()
            .ok_or(SupervisorError::UnknownConnectorType {
                id,
                type_id: row.connector_type_id,
            })?;
        let connector = self.instantiate(&row, connector_type)?;

        // Transition to Active and clear any prior error before spawning so
        // the runner starts from a clean lifecycle state.
        self.kg
            .set_connector_status(id, ConnectorStatus::Active, Some(None))
            .await?;

        self.spawn_into(row, connector_type, connector).await;
        Ok(())
    }

    /// Pause a connector: stop its runner and transition it to
    /// [`ConnectorStatus::Paused`] (Phase 3 A2 / #203).
    ///
    /// Stopping the runner first prevents a mid-cycle sync writing back to a
    /// row that is about to become `Paused`. The status write clears
    /// `last_error` so a paused connector does not display a stale error.
    /// A connector that was never running (no handle) is still transitioned
    /// to `Paused` so the persisted state reflects the request.
    pub async fn pause(&self, id: i32) -> Result<(), SupervisorError> {
        self.stop(id).await;
        self.kg
            .set_connector_status(id, ConnectorStatus::Paused, Some(None))
            .await?;
        Ok(())
    }

    /// Resume a paused/error connector: (re)spawn its runner and transition it
    /// to [`ConnectorStatus::Active`] (Phase 3 A2 / #203).
    ///
    /// Thin wrapper around [`start`](Self::start) that also covers the
    /// re-spawn-after-circuit-breaker case (an `Error` connector that
    /// exhausted its restart budget).
    pub async fn resume(&self, id: i32) -> Result<(), SupervisorError> {
        self.start(id).await
    }

    /// Dispatch a write-back action to a connector instance
    /// (Phase 3 A2 / #203, C4 / #198).
    ///
    /// Uses the live, running connector instance when one is available (so
    /// the action runs against the authenticated connector with its in-memory
    /// state). When no runner exists (a `Paused` / `Setup` / `Error`
    /// connector, or one whose runner exited), the connector is
    /// re-instantiated from its row — backends like the Calendar connector
    /// re-read credentials from the [`SecretStore`] inside `act`, so they do
    /// not depend on the runner's auth handshake. The connector's own
    /// [`ConnectorError`] (e.g. [`ConnectorError::UnsupportedAction`]) is
    /// returned for the server to map onto an HTTP status.
    pub async fn act(&self, id: i32, action: ConnectorAction) -> Result<ActionResult, ActError> {
        // Try the live handle first (no await while holding the lock).
        let live = {
            let guard = self.handles.lock().await;
            guard.get(&id).map(|h| Arc::clone(&h.connector))
        };
        let connector = match live {
            Some(c) => c,
            None => {
                let row = self
                    .kg
                    .get_connector(id)
                    .await?
                    .ok_or(ActError::NotFound(id))?;
                let connector_type = row.connector_type().ok_or(ActError::UnknownType {
                    id,
                    type_id: row.connector_type_id,
                })?;
                self.instantiate(&row, connector_type)
                    .map_err(|e| match e {
                        SupervisorError::Knowledge(ke) => ActError::Knowledge(ke),
                        SupervisorError::Connector(ce) => ActError::Connector(ce),
                        SupervisorError::Json(je) => {
                            ActError::Connector(ConnectorError::Config(je.to_string()))
                        }
                        SupervisorError::UnknownConnectorType { id, type_id } => {
                            ActError::UnknownType { id, type_id }
                        }
                    })?
            }
        };
        Ok(connector.act(action).await?)
    }

    /// Parse a row's `config_json`, inject instance identity, and ask the
    /// registry to construct the connector instance.
    fn instantiate(
        &self,
        row: &ConnectorRow,
        connector_type: ConnectorType,
    ) -> Result<Arc<dyn Connector>, SupervisorError> {
        let mut config: serde_json::Value = serde_json::from_str(&row.config_json)?;
        if let serde_json::Value::Object(map) = &mut config {
            map.insert("__slug".to_string(), serde_json::to_value(&row.slug)?);
            map.insert(
                "__ctype".to_string(),
                serde_json::to_value(connector_type as i16)?,
            );
            map.insert("__instance_id".to_string(), serde_json::to_value(row.id)?);
            // Inject the persisted sync cursor (C1 / #195) so incremental
            // connectors can seed their in-memory progress marker at
            // construction and skip already-processed items across restarts.
            // `None` is injected as JSON `null`, which connectors interpret as
            // "no prior cursor" (a full first scan).
            map.insert(
                "__cursor".to_string(),
                serde_json::to_value(&row.sync_cursor)?,
            );
        }
        Ok(self.registry.create_with_context(
            connector_type,
            &row.backend,
            config,
            &self.context,
        )?)
    }
}

// ---------------------------------------------------------------------------
// Runner task (one per active connector)
// ---------------------------------------------------------------------------

/// Outcome of a single sync cycle, returned from [`run_cycle`].
enum CycleOutcome {
    /// Cycle succeeded; cursor persisted, `last_error` cleared. Carries the
    /// connector's [`SyncOutcome`] so a triggered cycle can report stats back
    /// to the caller of [`ConnectorSupervisor::trigger_sync`].
    Ok(SyncOutcome),
    /// The service reported expired auth; the connector must be paused.
    AuthExpired,
    /// The cycle failed with a recoverable error.
    Err(String),
}

/// Classified result of awaiting a cycle's [`JoinHandle`].
enum CycleResult {
    Ok(SyncOutcome),
    AuthExpired,
    Err(String),
    /// The cycle task panicked (counted as a failure).
    Panic,
    /// The cycle task was cancelled without a panic (should not normally
    /// happen except on shutdown via abort).
    Cancelled,
    /// The shutdown signal fired mid-cycle; the in-flight cycle was aborted.
    Shutdown,
}

impl CycleResult {
    /// Map a cycle result onto the reply sent to a manual-sync caller
    /// (F9 / #186). Lifecycle side-effects (status writes, backoff) are
    /// applied by the runner separately; this only describes the outcome.
    fn to_trigger_outcome(&self) -> TriggerOutcome {
        match self {
            CycleResult::Ok(outcome) => TriggerOutcome::Ok {
                fetched: outcome.fetched,
                new_cursor: outcome.new_cursor.clone(),
            },
            CycleResult::AuthExpired => TriggerOutcome::AuthExpired,
            CycleResult::Err(message) => TriggerOutcome::Failed(message.clone()),
            CycleResult::Panic => TriggerOutcome::Failed("connector task panicked".to_string()),
            CycleResult::Cancelled => TriggerOutcome::Failed("cycle cancelled".to_string()),
            CycleResult::Shutdown => TriggerOutcome::Failed("shutdown".to_string()),
        }
    }
}

/// Event that starts the next cycle in a connector's runner loop.
enum NextEvent {
    /// Proceed with a default (incremental) cycle — the polling interval or
    /// backoff elapsed, or a push connector is looping immediately.
    Proceed,
    /// A manual sync trigger arrived; run a cycle with its [`SyncOptions`].
    Trigger(TriggerRequest),
    /// Shutdown was signalled (or the trigger channel closed).
    Shutdown,
}

/// Per-connector supervised loop.
///
/// Performs an initial auth handshake, then repeatedly: decide what should
/// start the next cycle (the polling interval, a manual sync trigger, or
/// shutdown), run one cycle in an isolated sub-task (so a connector panic
/// does not kill the runner) with the chosen [`SyncOptions`], classify the
/// result, apply backoff / circuit-breaker / auth-expiry / shutdown policy,
/// and reply to any waiting trigger caller.
async fn run_connector(
    connector: Arc<dyn Connector>,
    kg: Arc<KnowledgeGraph>,
    config: SupervisorConfig,
    mut shutdown: watch::Receiver<bool>,
    instance_id: i32,
    connector_type: ConnectorType,
    mut trigger_rx: mpsc::Receiver<TriggerRequest>,
) {
    // Initial auth handshake. A failed handshake pauses the connector; a
    // successful one persists the reported auth state.
    match connector.authenticate().await {
        Ok(state) => {
            if let Err(error) = kg.set_auth_state(instance_id, state).await {
                warn!(connector_id = instance_id, %error, "failed to persist auth state");
            }
        }
        Err(ConnectorError::NotAuthenticated) | Err(ConnectorError::Authentication(_)) => {
            warn!(
                connector_id = instance_id,
                "connector not authenticated; pausing"
            );
            let _ = kg
                .set_connector_status(
                    instance_id,
                    ConnectorStatus::Paused,
                    Some(Some("not authenticated".to_string())),
                )
                .await;
            return;
        }
        Err(error) => {
            warn!(connector_id = instance_id, %error, "auth handshake failed; continuing to first cycle");
        }
    }

    let mode = connector.mode();
    let mut failures: u32 = 0;
    let mut first_cycle = true;
    // Whether the previous cycle failed — selects backoff (preemptable by a
    // trigger) instead of the polling interval as the wait before the next
    // cycle.
    let mut last_failed = false;

    loop {
        // Authoritative shutdown check (catches a signal that arrived while the
        // previous select's `changed()` future was dropped without resolving).
        if *shutdown.borrow_and_update() {
            break;
        }

        // Decide the options (and optional trigger reply) for this cycle. The
        // first cycle runs immediately with default options; subsequent cycles
        // wait for the polling interval, a manual trigger, or shutdown.
        let (options, reply) = if first_cycle {
            first_cycle = false;
            (SyncOptions::default(), None)
        } else {
            match wait_next(
                &mode,
                &mut shutdown,
                &mut trigger_rx,
                last_failed,
                failures,
                config,
            )
            .await
            {
                NextEvent::Shutdown => break,
                NextEvent::Proceed => (SyncOptions::default(), None),
                NextEvent::Trigger(req) => {
                    info!(
                        connector_id = instance_id,
                        options = ?req.options,
                        "manual sync trigger received"
                    );
                    (req.options, Some(req.reply))
                }
            }
        };

        // Run one cycle in an isolated sub-task so a connector panic surfaces as
        // a `JoinError::is_panic` rather than unwinding the runner itself.
        let handle: JoinHandle<CycleOutcome> = tokio::spawn(run_cycle(
            connector.clone(),
            kg.clone(),
            instance_id,
            connector_type,
            options,
        ));
        let abort: AbortHandle = handle.abort_handle();

        let result = tokio::select! {
            res = handle => match res {
                Ok(CycleOutcome::Ok(outcome)) => CycleResult::Ok(outcome),
                Ok(CycleOutcome::AuthExpired) => CycleResult::AuthExpired,
                Ok(CycleOutcome::Err(message)) => CycleResult::Err(message),
                Err(join) if join.is_panic() => CycleResult::Panic,
                Err(_) => CycleResult::Cancelled,
            },
            _ = shutdown.changed() => {
                abort.abort();
                CycleResult::Shutdown
            }
        };

        // Report the outcome to a waiting trigger caller before applying
        // lifecycle policy that may exit the loop (so the caller is never
        // left hanging on a `RecvError`).
        if let Some(reply) = reply {
            let _ = reply.send(result.to_trigger_outcome());
        }

        match result {
            CycleResult::Ok(_) => {
                failures = 0;
                last_failed = false;
            }
            CycleResult::AuthExpired => {
                warn!(
                    connector_id = instance_id,
                    "connector auth expired; pausing"
                );
                let _ = kg
                    .set_auth_state(instance_id, ConnectorAuthState::Expired)
                    .await;
                let _ = kg
                    .set_connector_status(
                        instance_id,
                        ConnectorStatus::Paused,
                        Some(Some("auth expired".to_string())),
                    )
                    .await;
                return;
            }
            CycleResult::Err(message) => {
                failures += 1;
                last_failed = true;
                warn!(connector_id = instance_id, failures, error = %message, "connector cycle failed");
                if record_failure(&kg, instance_id, failures, config, &message).await {
                    return;
                }
                continue;
            }
            CycleResult::Panic => {
                failures += 1;
                last_failed = true;
                let message = "connector task panicked".to_string();
                warn!(
                    connector_id = instance_id,
                    failures, "connector cycle panicked"
                );
                if record_failure(&kg, instance_id, failures, config, &message).await {
                    return;
                }
                continue;
            }
            CycleResult::Cancelled => {
                warn!(
                    connector_id = instance_id,
                    "connector cycle cancelled without panic; stopping runner"
                );
                return;
            }
            CycleResult::Shutdown => break,
        }
    }

    info!(
        connector_id = instance_id,
        "connector runner exited on shutdown"
    );
}

/// Record a cycle failure: persist `last_error` as `Active` and trip the
/// circuit breaker once `failures` reaches `max_failures`.
///
/// Shared by the [`CycleResult::Err`] and [`CycleResult::Panic`] arms so the
/// breaker policy cannot drift between the sync-error and panic paths. The
/// exponential-backoff *wait* is not performed here — it is folded into
/// [`wait_next`] so a manual sync trigger can preempt it. Returns `true` when
/// the breaker has tripped (the caller should `return`); `false` otherwise
/// (the caller sets `last_failed` and `continue`s, and [`wait_next`] applies
/// the backoff).
async fn record_failure(
    kg: &KnowledgeGraph,
    instance_id: i32,
    failures: u32,
    config: SupervisorConfig,
    message: &str,
) -> bool {
    if failures >= config.max_failures {
        let _ = kg
            .set_connector_status(
                instance_id,
                ConnectorStatus::Error,
                Some(Some(message.to_string())),
            )
            .await;
        info!(
            connector_id = instance_id,
            failures, "circuit breaker tripped; connector moved to Error"
        );
        return true;
    }
    let _ = kg
        .set_connector_status(
            instance_id,
            ConnectorStatus::Active,
            Some(Some(message.to_string())),
        )
        .await;
    false
}

/// Wait for the event that should start the next cycle: the polling interval
/// elapsing, a manual sync trigger, or shutdown. After a failed cycle the wait
/// is exponential backoff (still preemptable by a trigger) instead of the
/// polling interval.
///
/// Push-mode connectors loop immediately on success (they block inside `sync`
/// waiting for service events, so there is no polling interval to wait on);
/// manual triggers are rejected upstream for push connectors, so the trigger
/// channel is never selected in the push success arm.
async fn wait_next(
    mode: &ConnectorMode,
    shutdown: &mut watch::Receiver<bool>,
    trigger_rx: &mut mpsc::Receiver<TriggerRequest>,
    last_failed: bool,
    failures: u32,
    config: SupervisorConfig,
) -> NextEvent {
    let delay = if last_failed {
        backoff_delay(config, failures)
    } else if let ConnectorMode::Polling { interval, jitter } = *mode {
        interval + jitter
    } else {
        // Push mode, successful last cycle: loop immediately.
        return NextEvent::Proceed;
    };

    tokio::select! {
        _ = tokio::time::sleep(delay) => NextEvent::Proceed,
        req = trigger_rx.recv() => match req {
            Some(req) => NextEvent::Trigger(req),
            None => NextEvent::Shutdown,
        },
        _ = shutdown.changed() => NextEvent::Shutdown,
    }
}

/// Exponential backoff delay: `min(base_backoff * 2^(failures-1), max_backoff)`.
///
/// The multiplier is clamped to avoid overflow; the product is saturated and
/// capped. Pure (non-async) so [`wait_next`] can compose the delay into a
/// `select!` that is preemptable by a trigger or shutdown.
fn backoff_delay(config: SupervisorConfig, failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(31);
    let multiplier = 2u32.saturating_pow(exponent);
    config
        .base_backoff
        .saturating_mul(multiplier)
        .min(config.max_backoff)
}

/// A single ingestion cycle, isolated in its own task for panic containment.
///
/// Health-probes, syncs (with the caller-supplied [`SyncOptions`]), extracts,
/// inserts through the shared pipeline, and persists the cursor. Returns a
/// [`CycleOutcome`] for the runner to act on.
async fn run_cycle(
    connector: Arc<dyn Connector>,
    kg: Arc<KnowledgeGraph>,
    instance_id: i32,
    connector_type: ConnectorType,
    options: SyncOptions,
) -> CycleOutcome {
    // Health probe — maps the transient status onto lifecycle decisions.
    match connector.health().await {
        Ok(HealthStatus::Online) | Ok(HealthStatus::Degraded) => {}
        Ok(HealthStatus::Offline) => {
            return CycleOutcome::Err("service offline".to_string());
        }
        Ok(HealthStatus::AuthExpired) => return CycleOutcome::AuthExpired,
        Ok(HealthStatus::NotConfigured) => return CycleOutcome::Err("not configured".to_string()),
        Err(error) => return CycleOutcome::Err(error.to_string()),
    }

    // Fetch raw items into the connector buffer.
    let outcome = match connector.sync(options).await {
        Ok(outcome) => outcome,
        Err(error) => return CycleOutcome::Err(error.to_string()),
    };

    // Drain the buffer into typed facts.
    let facts = match connector.extract().await {
        Ok(facts) => facts,
        Err(error) => return CycleOutcome::Err(error.to_string()),
    };

    // Insert through the shared pipeline (entity resolution, confidence,
    // sensitivity gate, corroboration/supersession inherited). Per-fact
    // errors are tolerated inside `normalize_and_insert`; a hard KG error
    // surfaces here and counts as a cycle failure.
    let provenance = Provenance::connector(
        instance_id,
        connector_type,
        ExtractionMethod::StructuredParse,
    );
    if let Err(error) = normalize_and_insert(&kg, facts, provenance).await {
        return CycleOutcome::Err(error.to_string());
    }

    // Persist sync progress so a mid-sync `mimir stop` does not re-fetch.
    //
    // `SyncOutcome::new_cursor` follows nullable-update semantics: `Some`
    // advances (or clears) the cursor, `None` means "unchanged". Passing
    // `None` to `update_sync_cursor` would *clear* the persisted cursor
    // (its `None`-clears contract), so we branch: a real cursor value goes
    // through `update_sync_cursor`; an unchanged cursor only stamps
    // `last_sync_at` via `touch_last_sync`, preserving the progress token.
    let persist = match outcome.new_cursor.as_deref() {
        Some(cursor) => kg.update_sync_cursor(instance_id, Some(cursor)).await,
        None => kg.touch_last_sync(instance_id).await,
    };
    if let Err(error) = persist {
        return CycleOutcome::Err(error.to_string());
    }

    // Success: clear any prior `last_error` and confirm Active.
    let _ = kg
        .set_connector_status(instance_id, ConnectorStatus::Active, Some(None))
        .await;

    CycleOutcome::Ok(outcome)
}

#[cfg(test)]
mod tests {
    //! Behavioural tests for config injection in [`ConnectorSupervisor::instantiate`].
    use super::*;
    use crate::FnConnectorFactory;
    use chrono::Utc;
    use mimir_knowledge::models::connector::Connector as ConnectorRow;
    use mimir_knowledge::models::connector::UpsertConnectorInput;

    fn row_with_cursor(cursor: Option<&str>) -> ConnectorRow {
        ConnectorRow {
            id: 7,
            connector_type_id: ConnectorType::Photos as i16,
            slug: "photos".to_string(),
            backend: "local".to_string(),
            display_name: "Photos".to_string(),
            config_json: "{}".to_string(),
            status_id: ConnectorStatus::Active as i16,
            auth_state_id: ConnectorAuthState::Authenticated as i16,
            sync_cursor: cursor.map(str::to_string),
            last_sync_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// `instantiate` must inject the persisted `sync_cursor` (alongside the
    /// existing identity keys) so incremental connectors can seed their
    /// in-memory cursor at construction (C1 / #195). A `None` cursor is
    /// injected as JSON `null`.
    #[tokio::test]
    async fn instantiate_injects_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("kg.db"))
                .await
                .unwrap(),
        );
        let captured = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
        let capture = captured.clone();
        let registry = ConnectorRegistry::new();
        registry
            .register(
                ConnectorType::Photos,
                "local".to_string(),
                FnConnectorFactory::new(move |config, _ctx| {
                    *capture.lock().unwrap() = Some(config.clone());
                    Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
                }),
            )
            .unwrap();
        let (_tx, rx) = watch::channel(false);
        let supervisor =
            ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);

        let connector = supervisor.instantiate(&row_with_cursor(Some("v1")), ConnectorType::Photos);
        assert!(connector.is_ok());

        let config = captured.lock().unwrap().take().expect("config captured");
        let map = config.as_object().expect("config is an object");
        assert_eq!(map.get("__slug").and_then(|v| v.as_str()), Some("photos"));
        // Derive the expected discriminant from the enum so the assertion
        // stays correct if `ConnectorType` ever changes its repr.
        assert_eq!(
            map.get("__ctype").and_then(|v| v.as_i64()),
            Some(ConnectorType::Photos as i64)
        );
        assert_eq!(map.get("__instance_id").and_then(|v| v.as_i64()), Some(7));
        assert_eq!(
            map.get("__cursor").and_then(|v| v.as_str()),
            Some("v1"),
            "persisted cursor must be injected for incremental connectors"
        );
    }

    #[tokio::test]
    async fn instantiate_injects_null_cursor_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("kg.db"))
                .await
                .unwrap(),
        );
        let captured = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
        let capture = captured.clone();
        let registry = ConnectorRegistry::new();
        registry
            .register(
                ConnectorType::Photos,
                "local".to_string(),
                FnConnectorFactory::new(move |config, _ctx| {
                    *capture.lock().unwrap() = Some(config.clone());
                    Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
                }),
            )
            .unwrap();
        let (_tx, rx) = watch::channel(false);
        let supervisor =
            ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx);

        // Assert the result instead of discarding it, so a construction
        // regression surfaces directly rather than failing later on the
        // opaque `expect("config captured")`.
        supervisor
            .instantiate(&row_with_cursor(None), ConnectorType::Photos)
            .expect("instantiate succeeds");
        let config = captured.lock().unwrap().take().expect("config captured");
        let map = config.as_object().expect("config is an object");
        assert!(
            map.get("__cursor").map(|v| v.is_null()).unwrap_or(false),
            "absent cursor must be injected as JSON null, not omitted"
        );
    }

    #[tokio::test]
    async fn with_secret_store_propagates_into_factory_context() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("kg.db"))
                .await
                .unwrap(),
        );
        // Capture whether the factory received a context carrying the store.
        let saw_store = Arc::new(std::sync::Mutex::new(false));
        let saw_store_cap = saw_store.clone();
        let registry = ConnectorRegistry::new();
        registry
            .register(
                ConnectorType::Calendar,
                "caldav".to_string(),
                FnConnectorFactory::new(move |_config, ctx| {
                    *saw_store_cap.lock().unwrap() = ctx.secret_store.is_some();
                    Ok(Arc::new(crate::MockConnector::default()) as Arc<dyn Connector>)
                }),
            )
            .unwrap();
        let (_tx, rx) = watch::channel(false);
        let store: Arc<dyn crate::secrets::SecretStore> =
            Arc::new(crate::InMemorySecretStore::new());
        let supervisor =
            ConnectorSupervisor::new(Arc::new(registry), kg, SupervisorConfig::default(), rx)
                .with_secret_store(store);

        let cal_row = ConnectorRow {
            id: 9,
            connector_type_id: ConnectorType::Calendar as i16,
            slug: "calendar-personal".to_string(),
            backend: "caldav".to_string(),
            display_name: "Calendar".to_string(),
            config_json: "{}".to_string(),
            status_id: ConnectorStatus::Active as i16,
            auth_state_id: ConnectorAuthState::Authenticated as i16,
            sync_cursor: None,
            last_sync_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        supervisor
            .instantiate(&cal_row, ConnectorType::Calendar)
            .expect("instantiate succeeds");
        assert!(
            *saw_store.lock().unwrap(),
            "with_secret_store must thread the store into the factory context"
        );
    }
    // -- start / pause / resume / act (Phase 3 A2 / #203) --

    /// Build a supervisor + KG with the mock factory registered, and insert a
    /// connector row in `Setup`/`Unauthenticated`. Returns the supervisor and
    /// the new row id.
    async fn supervisor_with_row(
        config_json: &str,
    ) -> (
        Arc<ConnectorSupervisor>,
        Arc<KnowledgeGraph>,
        i32,
        tempfile::TempDir,
        watch::Sender<bool>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("kg.db"))
                .await
                .unwrap(),
        );
        let registry = ConnectorRegistry::new();
        registry
            .register(
                ConnectorType::Gmail,
                "test".to_string(),
                crate::MockConnectorFactory,
            )
            .unwrap();
        // Keep the watch sender alive for the test's duration: dropping it
        // closes the channel, which the runner treats as a shutdown signal and
        // exits immediately.
        let (tx, rx) = watch::channel(false);
        let supervisor = ConnectorSupervisor::new(
            Arc::new(registry),
            Arc::clone(&kg),
            SupervisorConfig::default(),
            rx,
        );
        let row = kg
            .create_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Gmail,
                slug: "gmail-test".to_string(),
                backend: "test".to_string(),
                display_name: "Gmail Test".to_string(),
                config_json: config_json.to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();
        (Arc::new(supervisor), kg, row.id, dir, tx)
    }

    #[tokio::test]
    async fn start_spawns_runner_and_flips_active() {
        let (supervisor, kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
        // Starts in Setup (create_connector with status None).
        let before = kg.get_connector(id).await.unwrap().unwrap();
        assert_eq!(before.status(), Some(ConnectorStatus::Setup));

        supervisor.start(id).await.unwrap();

        // Give the runner a moment to run its auth handshake.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(supervisor.is_running(id).await);
        let after = kg.get_connector(id).await.unwrap().unwrap();
        assert_eq!(after.status(), Some(ConnectorStatus::Active));
        // Clean shutdown so the test does not leak a task.
        supervisor.stop(id).await;
    }

    #[tokio::test]
    async fn start_unknown_id_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("kg.db"))
                .await
                .unwrap(),
        );
        let registry = ConnectorRegistry::new();
        let (_tx, rx) = watch::channel(false);
        let supervisor = ConnectorSupervisor::new(
            Arc::new(registry),
            Arc::clone(&kg),
            SupervisorConfig::default(),
            rx,
        );
        let err = supervisor.start(9999).await.unwrap_err();
        assert!(matches!(
            err,
            SupervisorError::Knowledge(mimir_knowledge::KnowledgeError::ConnectorNotFound(9999))
        ));
    }

    #[tokio::test]
    async fn pause_stops_runner_and_flips_paused() {
        let (supervisor, kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
        supervisor.start(id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(supervisor.is_running(id).await);

        supervisor.pause(id).await.unwrap();

        assert!(!supervisor.is_running(id).await);
        let after = kg.get_connector(id).await.unwrap().unwrap();
        assert_eq!(after.status(), Some(ConnectorStatus::Paused));
    }

    #[tokio::test]
    async fn resume_respawns_after_pause() {
        let (supervisor, kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
        supervisor.start(id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        supervisor.pause(id).await.unwrap();
        assert!(!supervisor.is_running(id).await);
        assert_eq!(
            kg.get_connector(id).await.unwrap().unwrap().status(),
            Some(ConnectorStatus::Paused)
        );

        supervisor.resume(id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(supervisor.is_running(id).await);
        assert_eq!(
            kg.get_connector(id).await.unwrap().unwrap().status(),
            Some(ConnectorStatus::Active)
        );
        supervisor.stop(id).await;
    }

    #[tokio::test]
    async fn act_dispatches_to_live_connector() {
        let (supervisor, _kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
        supervisor.start(id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = supervisor
            .act(
                id,
                ConnectorAction {
                    kind: "echo".to_string(),
                    payload: serde_json::json!({
                        "native_id": "item-1",
                        "message": "ok",
                    }),
                },
            )
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.native_id.as_deref(), Some("item-1"));
        assert_eq!(result.message.as_deref(), Some("ok"));
        supervisor.stop(id).await;
    }

    #[tokio::test]
    async fn act_unsupported_kind_returns_error() {
        let (supervisor, _kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
        supervisor.start(id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let err = supervisor
            .act(
                id,
                ConnectorAction {
                    kind: "bogus".to_string(),
                    payload: serde_json::Value::Null,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ActError::Connector(ConnectorError::UnsupportedAction(_))
        ));
        supervisor.stop(id).await;
    }

    #[tokio::test]
    async fn act_reinstantiates_when_not_running() {
        // A connector that was never started has no live handle; act must
        // re-instantiate from the row and still dispatch.
        let (supervisor, _kg, id, _dir, _tx) = supervisor_with_row(r#"{"act_kind":"echo"}"#).await;
        // Note: no start() call — the connector is in Setup with no runner.
        let result = supervisor
            .act(
                id,
                ConnectorAction {
                    kind: "echo".to_string(),
                    payload: serde_json::json!({"native_id": "x"}),
                },
            )
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.native_id.as_deref(), Some("x"));
    }

    #[tokio::test]
    async fn act_unknown_id_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("kg.db"))
                .await
                .unwrap(),
        );
        let registry = ConnectorRegistry::new();
        let (_tx, rx) = watch::channel(false);
        let supervisor = ConnectorSupervisor::new(
            Arc::new(registry),
            Arc::clone(&kg),
            SupervisorConfig::default(),
            rx,
        );
        let err = supervisor
            .act(
                9999,
                ConnectorAction {
                    kind: "echo".to_string(),
                    payload: serde_json::Value::Null,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::NotFound(9999)));
    }
}
