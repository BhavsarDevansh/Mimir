use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::Connector as ConnectorRow;
use mimir_knowledge::models::enums::{ConnectorStatus, ConnectorType};

use crate::connector::{Connector, ConnectorContext, ConnectorMode};
use crate::registry::ConnectorRegistry;
use crate::secrets::SecretStore;
use mimir_core::geocoder::Geocoder;
use mimir_core::hooks::HookEngine;
use mimir_core::llm::LlmBackend;

use super::config::SupervisorConfig;
use super::cycle::{CycleRegistry, RunnerSignals, run_connector};
use super::error::SupervisorError;
use super::trigger::{TRIGGER_CHANNEL_CAPACITY, TriggerRequest};

pub(super) struct ConnectorHandle {
    /// The supervised runner task.
    pub(super) task: JoinHandle<()>,
    /// Per-runner stop signal. [`ConnectorSupervisor::stop`] sends `true` and
    /// then awaits the runner task, which aborts and awaits its in-flight
    /// cycle before exiting — so a stopped connector never leaves a cycle
    /// running (issue #266).
    pub(super) stop_tx: watch::Sender<bool>,
    /// The live connector instance (Phase 3 A2 / #203). Kept so
    /// [`ConnectorSupervisor::act`] can dispatch write-back actions to the
    /// running, authenticated instance without re-instantiating it. Cloned
    /// from the same `Arc<dyn Connector>` moved into the runner task, so both
    /// share one underlying instance.
    pub(super) connector: Arc<dyn Connector>,
    /// Connector mode, captured at spawn so `trigger_sync` can reject push
    /// connectors without holding the connector instance.
    pub(super) mode: ConnectorMode,
    /// Sender half of the per-connector trigger channel.
    pub(super) trigger_tx: mpsc::Sender<TriggerRequest>,
    /// One-permit semaphore serialising concurrent `trigger_sync` callers.
    pub(super) semaphore: Arc<Semaphore>,
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
    pub(super) registry: Arc<ConnectorRegistry>,
    pub(super) kg: Arc<KnowledgeGraph>,
    pub(super) config: SupervisorConfig,
    pub(super) shutdown: watch::Receiver<bool>,
    /// Shared services injected into every connector at construction (Phase 3
    /// C2 / #196 for the geocoder, C3 / #197 for the secret store). Built from
    /// [`with_geocoder`](Self::with_geocoder) and
    /// [`with_secret_store`](Self::with_secret_store); empty by default so
    /// connectors that need no injected services are unaffected.
    pub(super) context: ConnectorContext,
    pub(super) handles: Mutex<HashMap<i32, ConnectorHandle>>,
    /// In-flight cycle tasks for every live runner, keyed by instance id.
    /// Runners register each cycle's `JoinHandle` here before awaiting it and
    /// remove it when the cycle ends, so `shutdown()` can abort and await a
    /// cycle even when its runner had to be aborted — no cycle task outlives
    /// `shutdown` (issue #266).
    pub(super) cycle_tasks: CycleRegistry,
    /// Per-connector lifecycle locks serialising lifecycle mutations
    /// (`start` / `resume` vs the daemon's forget cascade) for one instance.
    /// Created on first use and retained; bounded by the number of connector
    /// ids ever operated on.
    pub(super) lifecycle_locks: Mutex<HashMap<i32, Arc<tokio::sync::Mutex<()>>>>,
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
            cycle_tasks: CycleRegistry::default(),
            lifecycle_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire the per-connector lifecycle lock for `id`.
    ///
    /// Serialises lifecycle mutations for one instance: `start` / `resume`,
    /// `pause`, and the daemon's forget cascade and connector-removal route
    /// all hold this lock, so a concurrent `resume` cannot re-spawn a runner
    /// while a cascade is deleting the row (and a cascade cannot trash facts
    /// or delete a row while a resume is mid-spawn), and a concurrent
    /// `pause` can never leave a `Paused` row with a live runner
    /// (issue #266). The lock is created on first use and retained.
    pub async fn lifecycle_lock(&self, id: i32) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.lifecycle_locks.lock().await;
            map.entry(id)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
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
    /// `__slug` injected into `config_json` by `Self::instantiate`).
    /// Must be called before [`restore`](Self::restore) so already-spawned runners receive
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
    /// ignored (treated as "no identity"). Must be called before [`restore`](Self::restore).
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
    /// call. Must be called before [`restore`](Self::restore) so already-spawned runners
    /// receive it. Connectors that need no LLM ignore it.
    ///
    /// [`restore`]: Self::restore
    pub fn with_llm_backend(mut self, backend: Arc<dyn LlmBackend>) -> Self {
        self.context.llm_backend = Some(backend);
        self
    }

    /// Inject the shared [`HookEngine`] made available to every connector
    /// this supervisor constructs (issue #386).
    ///
    /// The Email connector clones the engine out of the context and enqueues
    /// `ConnectorItemStaged` instances for prose emails that need LLM
    /// extraction. Must be called before [`restore`](Self::restore) so
    /// already-spawned runners receive it.
    pub fn with_hook_engine(mut self, engine: Arc<HookEngine>) -> Self {
        self.context.hook_engine = Some(engine);
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
    pub(super) async fn spawn_into(
        &self,
        row: ConnectorRow,
        connector_type: ConnectorType,
        connector: Arc<dyn Connector>,
    ) {
        // Enforce the documented invariant for every caller: an existing
        // runner is aborted and awaited before the new handle replaces it, so
        // a re-spawn never detaches a live task (dropping a `JoinHandle`
        // would leave the old runner polling untracked).
        self.stop(row.id).await;
        // Capture the mode up front so `trigger_sync` can reject push
        // connectors without holding the connector instance.
        let mode = connector.mode();
        let (trigger_tx, trigger_rx) = mpsc::channel(TRIGGER_CHANNEL_CAPACITY);
        let semaphore = Arc::new(Semaphore::new(1));
        let (stop_tx, stop_rx) = watch::channel(false);
        let handle = tokio::spawn(run_connector(
            // One clone feeds the runner; the other is retained below.
            Arc::clone(&connector),
            self.kg.clone(),
            self.config,
            RunnerSignals {
                shutdown: self.shutdown.clone(),
                stop: stop_rx,
            },
            row.id,
            connector_type,
            trigger_rx,
            self.cycle_tasks.clone(),
        ));
        self.handles.lock().await.insert(
            row.id,
            ConnectorHandle {
                task: handle,
                stop_tx,
                connector,
                mode,
                trigger_tx,
                semaphore,
            },
        );
        info!(connector_id = row.id, slug = %row.slug, backend = %row.backend, "spawned connector runner");
    }

    /// Clone the live connector for `id`, if its runner is still alive.
    ///
    /// A handle whose task has finished naturally (auth-expiry pause,
    /// circuit-breaker exhaustion, or panic) is stale: its in-memory
    /// connector may hold expired credentials, so it is dropped and `None` is
    /// returned so the caller re-instantiates from the row — mirroring
    /// [`trigger_sync`](Self::trigger_sync)'s `is_finished()` check. The
    /// lock is not held across any await: only the `Arc` clone (or a handle
    /// removal) happens inside the guard.
    pub(super) async fn live_connector(&self, id: i32) -> Option<Arc<dyn Connector>> {
        let mut guard = self.handles.lock().await;
        match guard.get(&id) {
            Some(handle) if !handle.task.is_finished() => Some(Arc::clone(&handle.connector)),
            Some(_) => {
                guard.remove(&id);
                None
            }
            None => None,
        }
    }

    /// Parse a row's `config_json`, inject instance identity, and ask the
    /// registry to construct the connector instance.
    pub(super) fn instantiate(
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
            // Inject the persisted durable state (issue #262) so connectors
            // that keep restart-safe state — e.g. the Email connector's
            // LLM-extraction retry ledger — can seed it at construction.
            // `None` is injected as JSON `null`, which connectors interpret
            // as "no durable state".
            map.insert(
                "__durable_state".to_string(),
                serde_json::to_value(&row.durable_state)?,
            );
        }
        // The connector-facing graph is always the supervisor's graph, so
        // connector facts and the connector rows their provenance references
        // can never land in different databases (issue #386 review).
        let mut context = self.context.clone();
        context.knowledge_graph = Some(Arc::clone(&self.kg));
        Ok(self
            .registry
            .create_with_context(connector_type, &row.backend, config, &context)?)
    }

    /// Resolve the mode a connector row would run in by constructing it from
    /// the persisted config with no side effects (issue #397) — the mode
    /// surfaced by `ConnectorResponse` (add summary and `mimir connector
    /// list`). Unknown connector types or invalid configs yield `None` so the
    /// response can omit the field.
    pub fn resolved_mode(&self, row: &ConnectorRow) -> Option<ConnectorMode> {
        let connector_type = row.connector_type()?;
        let connector = self.instantiate(row, connector_type).ok()?;
        Some(connector.mode())
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod control_tests;
#[cfg(test)]
#[path = "forget_tests.rs"]
mod forget_tests;
#[cfg(test)]
#[path = "instantiate_tests.rs"]
mod instantiate_tests;
