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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{info, warn};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::Connector as ConnectorRow;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::{Provenance, normalize_and_insert};

use crate::connector::ConnectorMode;
use crate::connector::{Connector, ConnectorError, HealthStatus, SyncOptions};
use crate::registry::ConnectorRegistry;

/// Tunable parameters for a [`ConnectorSupervisor`].
///
/// Injected at construction (no environment mutation, per the project safety
/// policy). Sensible defaults suit a single-user daemon; tests override with
/// millisecond values for fast, deterministic runs.
///
/// Exponential backoff here is *deterministic*: `base_backoff * 2^(n-1)`,
/// capped at `max_backoff`. Randomised jitter / rate-limit primitives belong
/// to F12 (issue #186) and are intentionally not re-implemented here.
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
    tasks: Mutex<HashMap<i32, JoinHandle<()>>>,
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
            tasks: Mutex::new(HashMap::new()),
        }
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
            let handle = tokio::spawn(run_connector(
                connector,
                self.kg.clone(),
                self.config,
                self.shutdown.clone(),
                row.id,
                connector_type,
            ));
            self.tasks.lock().await.insert(row.id, handle);
            spawned += 1;
            info!(connector_id = row.id, slug = %row.slug, backend = %row.backend, "spawned connector runner");
        }
        Ok(spawned)
    }

    /// Gracefully stop every runner: abort in-flight cycles and await exit.
    ///
    /// The shared shutdown `watch` is normally signalled first (so runners exit
    /// on their own); `abort` is a defensive fallback for stragglers.
    pub async fn shutdown(&self) {
        let handles: Vec<JoinHandle<()>> =
            self.tasks.lock().await.drain().map(|(_, h)| h).collect();
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Number of runner tasks that are still alive (not yet finished).
    pub async fn running_count(&self) -> usize {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|handle| !handle.is_finished())
            .count()
    }

    /// Whether the runner for `id` is still alive.
    pub async fn is_running(&self, id: i32) -> bool {
        self.tasks
            .lock()
            .await
            .get(&id)
            .is_some_and(|handle| !handle.is_finished())
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
        }
        Ok(self.registry.create(connector_type, &row.backend, config)?)
    }
}

// ---------------------------------------------------------------------------
// Runner task (one per active connector)
// ---------------------------------------------------------------------------

/// Outcome of a single sync cycle, returned from [`run_cycle`].
enum CycleOutcome {
    /// Cycle succeeded; cursor persisted, `last_error` cleared.
    Ok,
    /// The service reported expired auth; the connector must be paused.
    AuthExpired,
    /// The cycle failed with a recoverable error.
    Err(String),
}

/// Classified result of awaiting a cycle's [`JoinHandle`].
enum CycleResult {
    Ok,
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

/// Per-connector supervised loop.
///
/// Performs an initial auth handshake, then repeatedly: run one cycle in an
/// isolated sub-task (so a connector panic does not kill the runner), classify
/// the result, apply backoff / circuit-breaker / auth-expiry / shutdown policy,
/// and (for polling connectors) sleep the declared interval before the next
/// cycle.
async fn run_connector(
    connector: Arc<dyn Connector>,
    kg: Arc<KnowledgeGraph>,
    config: SupervisorConfig,
    mut shutdown: watch::Receiver<bool>,
    instance_id: i32,
    connector_type: ConnectorType,
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

    loop {
        // Authoritative shutdown check (catches a signal that arrived while the
        // previous select's `changed()` future was dropped without resolving).
        if *shutdown.borrow_and_update() {
            break;
        }

        // Run one cycle in an isolated sub-task so a connector panic surfaces as
        // a `JoinError::is_panic` rather than unwinding the runner itself.
        let handle: JoinHandle<CycleOutcome> = tokio::spawn(run_cycle(
            connector.clone(),
            kg.clone(),
            instance_id,
            connector_type,
        ));
        let abort: AbortHandle = handle.abort_handle();

        let result = tokio::select! {
            res = handle => match res {
                Ok(CycleOutcome::Ok) => CycleResult::Ok,
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

        match result {
            CycleResult::Ok => {
                failures = 0;
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
                warn!(connector_id = instance_id, failures, error = %message, "connector cycle failed");
                if record_failure(&kg, instance_id, failures, config, &message, &mut shutdown).await
                {
                    return;
                }
                continue;
            }
            CycleResult::Panic => {
                failures += 1;
                let message = "connector task panicked".to_string();
                warn!(
                    connector_id = instance_id,
                    failures, "connector cycle panicked"
                );
                if record_failure(&kg, instance_id, failures, config, &message, &mut shutdown).await
                {
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

        // Polling connectors wait for their declared interval (+ jitter) before
        // the next cycle. Push connectors block inside `sync`, so they loop
        // immediately. The sleep is cancelled by the shutdown signal.
        if let ConnectorMode::Polling { interval, jitter } = mode {
            let delay = interval + jitter;
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = shutdown.changed() => break,
            }
        }
    }

    info!(
        connector_id = instance_id,
        "connector runner exited on shutdown"
    );
}

/// Record a cycle failure: persist `last_error` as `Active`, apply exponential
/// backoff, and trip the circuit breaker once `failures` reaches `max_failures`.
///
/// Shared by the [`CycleResult::Err`] and [`CycleResult::Panic`] arms so the
/// breaker / backoff policy cannot drift between the sync-error and panic paths.
/// Returns `true` when the breaker has tripped (the caller should `return`);
/// `false` after backoff (the caller should `continue`).
async fn record_failure(
    kg: &KnowledgeGraph,
    instance_id: i32,
    failures: u32,
    config: SupervisorConfig,
    message: &str,
    shutdown: &mut watch::Receiver<bool>,
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
    backoff_sleep(config, failures, shutdown).await;
    false
}

/// A single ingestion cycle, isolated in its own task for panic containment.
///
/// Health-probes, syncs, extracts, inserts through the shared pipeline, and
/// persists the cursor. Returns a [`CycleOutcome`] for the runner to act on.
async fn run_cycle(
    connector: Arc<dyn Connector>,
    kg: Arc<KnowledgeGraph>,
    instance_id: i32,
    connector_type: ConnectorType,
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
    let outcome = match connector
        .sync(SyncOptions {
            full: false,
            since: None,
        })
        .await
    {
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

    CycleOutcome::Ok
}

/// Exponential backoff sleep, cancelled by the shutdown signal.
///
/// `delay = min(base_backoff * 2^(failures-1), max_backoff)`. The multiplier is
/// clamped to avoid overflow; the product is saturated and capped.
async fn backoff_sleep(
    config: SupervisorConfig,
    failures: u32,
    shutdown: &mut watch::Receiver<bool>,
) {
    let exponent = failures.saturating_sub(1).min(31);
    let multiplier = 2u32.saturating_pow(exponent);
    let delay = config
        .base_backoff
        .saturating_mul(multiplier)
        .min(config.max_backoff);
    tokio::select! {
        _ = tokio::time::sleep(delay) => {}
        _ = shutdown.changed() => {}
    }
}
