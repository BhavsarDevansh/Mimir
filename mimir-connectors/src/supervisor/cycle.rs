use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{info, warn};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::{Provenance, normalize_and_insert};

use crate::connector::{
    Connector, ConnectorError, ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};

use super::config::SupervisorConfig;
use super::trigger::{TriggerOutcome, TriggerRequest};

/// The signals a runner observes: the daemon-wide shutdown channel and the
/// per-runner stop channel (issue #266). Grouped so [`run_connector`] and
/// [`wait_next`] take one signals argument instead of two receivers.
pub(super) struct RunnerSignals {
    pub(super) shutdown: watch::Receiver<bool>,
    pub(super) stop: watch::Receiver<bool>,
}

/// Aborts the in-flight cycle sub-task when the runner task is dropped.
///
/// The runner runs each cycle in an isolated sub-task so a connector panic
/// cannot unwind the runner. When the runner task is aborted (the
/// `ConnectorSupervisor::shutdown` fallback path), the sub-task would
/// otherwise be detached and keep running to completion — overlapping the
/// next runner's cycle and writing facts after the connector was stopped.
/// Holding the cycle's [`AbortHandle`] in a guard that aborts on [`Drop`]
/// propagates the runner's cancellation to the in-flight cycle (issue #266).
struct CycleAbortGuard {
    abort: Option<AbortHandle>,
}

impl CycleAbortGuard {
    fn new(abort: AbortHandle) -> Self {
        Self { abort: Some(abort) }
    }

    /// Disarm after the cycle completed normally, so dropping the guard does
    /// not abort a finished task.
    fn disarm(&mut self) {
        self.abort = None;
    }
}

impl Drop for CycleAbortGuard {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
    }
}

/// Registry of in-flight cycle tasks shared with the supervisor, keyed by
/// instance id.
///
/// The runner registers each cycle's [`JoinHandle`] before awaiting it and
/// removes the entry once the cycle ends. `ConnectorSupervisor::shutdown`
/// drains the registry after runners exit so a cycle whose runner had to be
/// aborted (the straggler fallback) is still aborted and awaited — a
/// `JoinHandle` dropped un-awaited would let the cycle task outlive
/// `shutdown` and keep writing facts after teardown (issue #266).
#[derive(Clone, Default)]
pub(super) struct CycleRegistry {
    tasks: Arc<Mutex<HashMap<i32, JoinHandle<CycleOutcome>>>>,
}

impl CycleRegistry {
    /// Register `handle` for `instance_id`, replacing any previous entry.
    pub(super) async fn insert(&self, instance_id: i32, handle: JoinHandle<CycleOutcome>) {
        self.tasks.lock().await.insert(instance_id, handle);
    }

    /// Remove and return the registered handle for `instance_id`.
    pub(super) async fn remove(&self, instance_id: i32) -> Option<JoinHandle<CycleOutcome>> {
        self.tasks.lock().await.remove(&instance_id)
    }

    /// Drain every registered cycle handle (used by `shutdown`'s fallback).
    pub(super) async fn drain(&self) -> Vec<JoinHandle<CycleOutcome>> {
        self.tasks
            .lock()
            .await
            .drain()
            .map(|(_, handle)| handle)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Runner task (one per active connector)
// ---------------------------------------------------------------------------

/// Outcome of a single sync cycle, returned from [`run_cycle`].
pub(super) enum CycleOutcome {
    /// Cycle succeeded; cursor persisted, `last_error` cleared. Carries the
    /// connector's [`SyncOutcome`] so a triggered cycle can report stats back
    /// to the caller of [`ConnectorSupervisor::trigger_sync`].
    Ok(SyncOutcome),
    /// The service reported expired, revoked, or rejected auth; the connector
    /// must be paused. Carries the underlying auth rejection message so it
    /// can be logged and persisted as `last_error` (issue #507).
    AuthExpired(String),
    /// The cycle failed with a recoverable error.
    Err(String),
}

/// Classified result of awaiting a cycle's [`JoinHandle`].
pub(super) enum CycleResult {
    Ok(SyncOutcome),
    AuthExpired(String),
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
            CycleResult::AuthExpired(message) => TriggerOutcome::AuthExpired(message.clone()),
            CycleResult::Err(message) => TriggerOutcome::Failed(message.clone()),
            CycleResult::Panic => TriggerOutcome::Failed("connector task panicked".to_string()),
            CycleResult::Cancelled => TriggerOutcome::Failed("cycle cancelled".to_string()),
            CycleResult::Shutdown => TriggerOutcome::Failed("shutdown".to_string()),
        }
    }
}

/// Event that starts the next cycle in a connector's runner loop.
pub(super) enum NextEvent {
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
/// the daemon-wide shutdown or the per-runner stop signal), run one cycle in
/// an isolated sub-task (so a connector panic does not kill the runner) with
/// the chosen [`SyncOptions`], classify the result, apply backoff /
/// circuit-breaker / auth-expiry / shutdown policy, and reply to any waiting
/// trigger caller.
#[allow(clippy::too_many_arguments)] // each arg is a distinct runner input (connector, services, signals, channels, cycle registry)
pub(super) async fn run_connector(
    connector: Arc<dyn Connector>,
    kg: Arc<KnowledgeGraph>,
    config: SupervisorConfig,
    mut signals: RunnerSignals,
    instance_id: i32,
    connector_type: ConnectorType,
    mut trigger_rx: mpsc::Receiver<TriggerRequest>,
    cycles: CycleRegistry,
) {
    // Initial auth handshake. A failed handshake pauses the connector; a
    // successful one persists the reported auth state. The handshake is
    // preemptable by the daemon-wide shutdown and the per-runner stop
    // signal: without the select, a slow or hung handshake (e.g. an
    // unreachable IMAP/CalDAV server) would block `ConnectorSupervisor::stop`
    // — which awaits this task — for the whole handshake (issue #266).
    let auth = tokio::select! {
        state = connector.authenticate() => state,
        _ = signals.shutdown.changed() => return,
        _ = signals.stop.changed() => return,
    };
    match auth {
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
        if *signals.shutdown.borrow_and_update() || *signals.stop.borrow_and_update() {
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
                &mut signals,
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
        // If this runner task is itself aborted (the `shutdown()` fallback
        // path), abort the in-flight cycle instead of detaching it — a
        // detached cycle would keep syncing and writing facts after the
        // connector was stopped (issue #266).
        let mut cycle_abort = CycleAbortGuard::new(abort.clone());
        // Register the cycle handle with the supervisor before awaiting it so
        // `shutdown()` can still abort and await it if this runner is itself
        // aborted mid-cycle; the registry lock is held across the await so
        // the handle is always reachable.
        cycles.insert(instance_id, handle).await;

        let result = {
            let mut tasks = cycles.tasks.lock().await;
            let handle = tasks
                .get_mut(&instance_id)
                .expect("cycle handle must be registered before awaiting");
            tokio::select! {
                res = &mut *handle => match res {
                    Ok(CycleOutcome::Ok(outcome)) => CycleResult::Ok(outcome),
                    Ok(CycleOutcome::AuthExpired(message)) => CycleResult::AuthExpired(message),
                    Ok(CycleOutcome::Err(message)) => CycleResult::Err(message),
                    Err(join) if join.is_panic() => CycleResult::Panic,
                    Err(_) => CycleResult::Cancelled,
                },
                _ = signals.shutdown.changed() => CycleResult::Shutdown,
                _ = signals.stop.changed() => CycleResult::Shutdown,
            }
        };
        cycle_abort.disarm();

        // The cycle ended: take the handle back out of the registry. It stays
        // registered while the runner is alive so `shutdown()`'s registry
        // drain can always reach it, even if this runner is aborted mid-cycle.
        let handle = cycles
            .remove(instance_id)
            .await
            .expect("cycle handle must remain registered until the cycle ends");
        if matches!(result, CycleResult::Shutdown) {
            // Abort the sub-task and await its termination before exiting, so
            // the caller of `stop` (which awaits this runner) knows no cycle
            // is still running (issue #266).
            abort.abort();
            // Await only when the cycle is still running: if it completed in
            // the same instant the shutdown signal won the select, the handle
            // was already polled to completion and must not be polled again.
            if !handle.is_finished() {
                let _ = handle.await;
            }
        }

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
            CycleResult::AuthExpired(message) => {
                warn!(
                    connector_id = instance_id,
                    error = %message,
                    "connector auth expired; pausing"
                );
                let _ = kg
                    .set_auth_state(instance_id, ConnectorAuthState::Expired)
                    .await;
                let _ = kg
                    .set_connector_status(instance_id, ConnectorStatus::Paused, Some(Some(message)))
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
pub(super) async fn record_failure(
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
/// elapsing, a manual sync trigger, the daemon-wide shutdown, or the
/// per-runner stop signal. After a failed cycle the wait is exponential
/// backoff (still preemptable by a trigger) instead of the polling interval.
///
/// Push-mode connectors loop immediately on success (they block inside `sync`
/// waiting for service events, so there is no polling interval to wait on).
/// The trigger channel is still drained in the push success arm: the
/// manual-sync gate accepts triggers for an `auto` connector whose capability
/// probe has not completed (issue #475), and the probe can resolve to push
/// after the gate check but before the runner reads the channel — a queued
/// trigger must run its cycle, not strand the awaiting caller.
pub(super) async fn wait_next(
    mode: &ConnectorMode,
    signals: &mut RunnerSignals,
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
        // Push mode, successful last cycle: loop immediately — but drain any
        // trigger queued while the mode was unprobed (issue #475). The gate
        // accepts triggers for an `auto` connector whose capability probe has
        // not completed, and the probe can resolve to push after the gate
        // check but before the runner reads the channel; a queued trigger must
        // run its cycle, not strand the awaiting caller. `try_recv` is
        // non-blocking, so the busy loop is unchanged when the channel is
        // empty.
        return match trigger_rx.try_recv() {
            Ok(req) => NextEvent::Trigger(req),
            Err(mpsc::error::TryRecvError::Empty) => NextEvent::Proceed,
            Err(mpsc::error::TryRecvError::Disconnected) => NextEvent::Shutdown,
        };
    };

    tokio::select! {
        _ = tokio::time::sleep(delay) => NextEvent::Proceed,
        req = trigger_rx.recv() => match req {
            Some(req) => NextEvent::Trigger(req),
            None => NextEvent::Shutdown,
        },
        _ = signals.shutdown.changed() => NextEvent::Shutdown,
        _ = signals.stop.changed() => NextEvent::Shutdown,
    }
}

/// Exponential backoff delay: `min(base_backoff * 2^(failures-1), max_backoff)`.
///
/// The multiplier is clamped to avoid overflow; the product is saturated and
/// capped. Pure (non-async) so [`wait_next`] can compose the delay into a
/// `select!` that is preemptable by a trigger or shutdown.
pub(super) fn backoff_delay(config: SupervisorConfig, failures: u32) -> Duration {
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
pub(super) async fn run_cycle(
    connector: Arc<dyn Connector>,
    kg: Arc<KnowledgeGraph>,
    instance_id: i32,
    connector_type: ConnectorType,
    options: SyncOptions,
) -> CycleOutcome {
    // Health probe — maps the transient status onto lifecycle decisions.
    // Issue #507: an auth rejection is probed twice at most. The first probe
    // runs one forced refresh (OAuth connectors with a valid refresh token)
    // and re-probes with the fresh credential, so a stale or transiently
    // rejected access token is retried instead of pausing the connector; only
    // a second rejection — or a refresh failure — pauses, carrying the actual
    // auth error message for `last_error` and the logs.
    let mut auth_retry = true;
    loop {
        match connector.health().await {
            Ok(HealthStatus::Online) | Ok(HealthStatus::Degraded) => break,
            Ok(HealthStatus::Offline) => {
                return CycleOutcome::Err("service offline".to_string());
            }
            Ok(HealthStatus::AuthExpired(message)) if auth_retry => {
                auth_retry = false;
                match connector.force_refresh().await {
                    Ok(ConnectorAuthState::Authenticated) => {
                        info!(
                            connector_id = instance_id,
                            "connector auth rejected; forced refresh succeeded, retrying the cycle"
                        );
                        continue;
                    }
                    Ok(_) => return CycleOutcome::AuthExpired(message),
                    // A transient refresh failure (network / malformed token
                    // response) is a recoverable cycle error: the supervisor
                    // backs off and retries, and a later cycle can still
                    // recover. Only an auth-level refresh rejection (e.g.
                    // `invalid_grant` — a revoked refresh token) pauses,
                    // carrying the provider's message (issue #507 review).
                    Err(ConnectorError::Network(message)) => {
                        return CycleOutcome::Err(message);
                    }
                    Err(ConnectorError::Parse(message)) => {
                        return CycleOutcome::Err(message);
                    }
                    Err(error) => return CycleOutcome::AuthExpired(error.to_string()),
                }
            }
            Ok(HealthStatus::AuthExpired(message)) => return CycleOutcome::AuthExpired(message),
            Ok(HealthStatus::NotConfigured) => {
                return CycleOutcome::Err("not configured".to_string());
            }
            Err(error) => return CycleOutcome::Err(error.to_string()),
        }
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

    // Report server-side removals (tombstones) and trash the matching facts
    // this instance authored. Processed *before* inserting this cycle's
    // facts so a raw item that was deleted and re-created within one window
    // ends up represented by the fresh facts rather than trashed after
    // insertion. `extract_deletions` is non-destructive: the removals are
    // acknowledged via `acknowledge_deletions` only after trashing, fact
    // insertion, and cursor persistence all succeeded, so any failure leaves
    // them pending and the next cycle re-reports them (the connector's
    // in-memory token may have advanced, but the pending buffer still
    // replays the removal; a restart resumes from the un-persisted cursor).
    let deletions = match connector.extract_deletions().await {
        Ok(deletions) => deletions,
        Err(error) => return CycleOutcome::Err(error.to_string()),
    };
    if !deletions.is_empty() {
        if let Err(error) = kg
            .forget_connector_facts_by_raw_reference(instance_id, &deletions, ChangedBy::System)
            .await
        {
            return CycleOutcome::Err(error.to_string());
        }
    }

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

    // Persist sync progress and connector-side durable state (e.g. the
    // Email connector's bounded LLM-extraction retry ledger, issue #262) in
    // **one transaction** so a mid-sync `mimir stop` does not re-fetch, and
    // a crash between the two writes cannot advance the cursor without its
    // retry record (PR #318 review): a restart would otherwise skip the
    // failed message because the cursor advanced without its durable state.
    //
    // `SyncOutcome::new_cursor` follows nullable-update semantics: `Some`
    // advances (or clears) the cursor, `None` means "unchanged". Passing
    // `None` to `update_sync_cursor` would *clear* the persisted cursor
    // (its `None`-clears contract), so the combined persist uses
    // `None`-means-unchanged semantics: a real cursor value advances the
    // cursor; an unchanged cursor only stamps `last_sync_at`, preserving the
    // progress token. `durable_state` is `None` when the connector reports
    // no change (no write). The connector only acknowledges the persist
    // (`durable_state_persisted`) after the combined commit succeeds, so a
    // failed write leaves the connector's state dirty and the next cycle
    // re-writes it instead of silently losing it.
    let durable_state = connector.durable_state();
    let persist = match outcome.new_cursor.as_deref() {
        Some(cursor) => {
            kg.update_sync_progress_and_durable_state(
                instance_id,
                Some(cursor),
                durable_state.as_deref(),
            )
            .await
        }
        None => {
            kg.update_sync_progress_and_durable_state(instance_id, None, durable_state.as_deref())
                .await
        }
    };
    if let Err(error) = persist {
        return CycleOutcome::Err(error.to_string());
    }
    if durable_state.is_some() {
        connector.durable_state_persisted();
    }

    // The cursor (and any durable state) is persisted — only now may the
    // connector adopt it as its in-memory progress marker. The adoption is
    // deliberately deferred past `sync`/`extract`/insert/persist so a cycle
    // that fails part-way re-syncs from the last confirmed cursor on the
    // next in-process cycle instead of skipping the failed window (issue
    // #314). The call returns no error: even if the connector's internal
    // adoption does not take effect, the DB cursor is already committed and
    // the next cycle re-syncs from the stale in-memory marker (an idempotent
    // re-statement, never data loss).
    connector
        .on_cycle_succeeded(outcome.new_cursor.as_deref())
        .await;

    // The deletions were trashed, this cycle's facts inserted, and the cursor
    // persisted — only now may the connector drop the acknowledged removals.
    // An acknowledgement failure is not fatal: the removals stay pending and
    // are re-reported next cycle, where the idempotent trash path makes the
    // re-processing a no-op.
    if !deletions.is_empty() {
        if let Err(error) = connector.acknowledge_deletions(&deletions).await {
            warn!(
                %error,
                "failed to acknowledge processed deletions; they will be re-reported"
            );
        }
    }

    // Success: clear any prior `last_error` and confirm Active.
    let _ = kg
        .set_connector_status(instance_id, ConnectorStatus::Active, Some(None))
        .await;

    CycleOutcome::Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use serde_json::json;

    use mimir_knowledge::models::connector::UpsertConnectorInput;
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

    use crate::MockConnector;

    /// Delegating connector that fails `extract_deletions` on its first call,
    /// simulating a transient deletion-processing failure. The cycle-failure
    /// boundary is the same as a knowledge-graph trash error — both return
    /// before the supervisor acknowledges the deletions — so the retention
    /// and replay behaviour under test is identical.
    struct FlakyDeletionConnector {
        inner: MockConnector,
        failures: AtomicU32,
    }

    #[async_trait::async_trait]
    impl Connector for FlakyDeletionConnector {
        fn id(&self) -> &str {
            self.inner.id()
        }
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn connector_type(&self) -> ConnectorType {
            self.inner.connector_type()
        }
        fn mode(&self) -> ConnectorMode {
            self.inner.mode()
        }
        fn config_schema(&self) -> serde_json::Value {
            self.inner.config_schema()
        }
        async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
            self.inner.authenticate().await
        }
        async fn health(&self) -> Result<HealthStatus, ConnectorError> {
            self.inner.health().await
        }
        async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
            self.inner.sync(options).await
        }
        async fn extract(
            &self,
        ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
            self.inner.extract().await
        }
        async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
            if self.failures.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(ConnectorError::Network(
                    "transient deletion-processing failure".to_string(),
                ));
            }
            self.inner.extract_deletions().await
        }
        async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
            self.inner.acknowledge_deletions(deleted).await
        }
        async fn act(
            &self,
            action: crate::connector::ConnectorAction,
        ) -> Result<crate::connector::ActionResult, ConnectorError> {
            self.inner.act(action).await
        }
        async fn forget(&self) -> Result<(), ConnectorError> {
            self.inner.forget().await
        }
    }

    /// PR #313 review: a transient deletion-processing failure must not lose
    /// the staged tombstones. `extract_deletions` is non-destructive and the
    /// acknowledgement only happens after the cycle succeeds, so the next
    /// in-process cycle re-reports the removal and the fact is trashed.
    #[tokio::test]
    async fn failed_deletion_processing_is_replayed_on_the_next_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );

        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "flaky-del".to_string(),
                backend: "mock".to_string(),
                display_name: "Flaky Del".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();

        let subject = kg
            .create_entity("Alice Flaky", EntityType::Person, &[])
            .await
            .unwrap();
        let object = kg
            .create_entity("Acme", EntityType::Organization, &[])
            .await
            .unwrap();
        let fact = kg
            .insert_fact(NewFact {
                subject_id: subject.id,
                relationship_type: "works_at".to_string(),
                object_id: Some(object.id),
                object_literal: None,
                valid_from: None,
                valid_until: None,
                source_type: SourceType::Connector,
                connector_instance_id: Some(row.id),
                connector_type: Some(ConnectorType::Email),
                raw_reference: Some("del-1".to_string()),
                extraction_method: Some(ExtractionMethod::StructuredParse),
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: Vec::new(),
                category_ids: Vec::new(),
            })
            .await
            .unwrap();

        let connector = Arc::new(FlakyDeletionConnector {
            inner: MockConnector::from_config(json!({
                "__slug": "flaky-del",
                "mode": "polling",
                "interval_ms": 1,
                "jitter_ms": 0,
                "deletions": ["del-1"],
            }))
            .unwrap(),
            failures: AtomicU32::new(0),
        });

        // Cycle 1: the deletion drain fails after `sync` staged the
        // tombstone; the cycle errors without an acknowledgement.
        let first = run_cycle(
            connector.clone(),
            kg.clone(),
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        assert!(matches!(first, CycleOutcome::Err(_)));

        // Cycle 2: the retained tombstone is re-reported, trashed, and then
        // acknowledged — the fact is gone despite the earlier failure.
        let second = run_cycle(
            connector.clone(),
            kg.clone(),
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        assert!(matches!(second, CycleOutcome::Ok(_)));
        assert!(
            kg.get_fact(fact.id).await.unwrap().is_none(),
            "the tombstoned fact must be trashed on the retry cycle"
        );
        assert!(
            connector.extract_deletions().await.unwrap().is_empty(),
            "the acknowledged tombstones are dropped after a successful cycle"
        );
    }

    /// How a test connector's forced refresh resolves (issue #507): recover
    /// (the refresh fixes the credential), unchanged (no refresh possible,
    /// e.g. an app-password connector), fail with the provider's auth error,
    /// or fail with a transient network error.
    #[derive(Clone, Copy, PartialEq)]
    enum RefreshOutcome {
        Recover,
        Unchanged,
        Fail,
        NetworkFail,
    }

    /// Delegating connector whose health probe reports `AuthExpired` on its
    /// first call and then delegates to the wrapped mock, simulating an OAuth
    /// connector whose access token was transiently rejected. The forced
    /// refresh resolves per [`RefreshOutcome`].
    struct ForceRefreshConnector {
        inner: MockConnector,
        probes: AtomicU32,
        outcome: RefreshOutcome,
    }

    #[async_trait::async_trait]
    impl Connector for ForceRefreshConnector {
        fn id(&self) -> &str {
            self.inner.id()
        }
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn connector_type(&self) -> ConnectorType {
            self.inner.connector_type()
        }
        fn mode(&self) -> ConnectorMode {
            self.inner.mode()
        }
        fn config_schema(&self) -> serde_json::Value {
            self.inner.config_schema()
        }
        async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
            self.inner.authenticate().await
        }
        async fn health(&self) -> Result<HealthStatus, ConnectorError> {
            if self.probes.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(HealthStatus::AuthExpired(
                    "IMAP auth rejected (BAD): invalid token".to_string(),
                ));
            }
            self.inner.health().await
        }
        async fn force_refresh(&self) -> Result<ConnectorAuthState, ConnectorError> {
            match self.outcome {
                RefreshOutcome::Recover => Ok(ConnectorAuthState::Authenticated),
                RefreshOutcome::Unchanged => Ok(ConnectorAuthState::Expired),
                RefreshOutcome::Fail => Err(ConnectorError::Authentication(
                    "token refresh failed: invalid_grant: refresh token revoked".to_string(),
                )),
                RefreshOutcome::NetworkFail => Err(ConnectorError::Network(
                    "token refresh failed: connection refused".to_string(),
                )),
            }
        }
        async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
            self.inner.sync(options).await
        }
        async fn extract(
            &self,
        ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
            self.inner.extract().await
        }
        async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
            self.inner.extract_deletions().await
        }
        async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
            self.inner.acknowledge_deletions(deleted).await
        }
        async fn act(
            &self,
            action: crate::connector::ConnectorAction,
        ) -> Result<crate::connector::ActionResult, ConnectorError> {
            self.inner.act(action).await
        }
        async fn forget(&self) -> Result<(), ConnectorError> {
            self.inner.forget().await
        }
    }

    /// Issue #507: a single auth rejection must not pause an OAuth connector
    /// whose forced refresh succeeds — the cycle is re-probed with the fresh
    /// credential and proceeds.
    #[tokio::test]
    async fn auth_expired_recovers_via_forced_refresh_and_runs_the_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );
        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "oauth-recover".to_string(),
                backend: "mock".to_string(),
                display_name: "OAuth Recover".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();
        let connector = Arc::new(ForceRefreshConnector {
            inner: MockConnector::from_config(json!({
                "__slug": "oauth-recover",
                "mode": "polling",
                "interval_ms": 1,
                "jitter_ms": 0,
            }))
            .unwrap(),
            probes: AtomicU32::new(0),
            outcome: RefreshOutcome::Recover,
        });

        let outcome = run_cycle(
            connector.clone(),
            kg.clone(),
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        assert!(
            matches!(outcome, CycleOutcome::Ok(_)),
            "a successful forced refresh must retry the cycle instead of pausing"
        );
        assert_eq!(
            connector.probes.load(Ordering::SeqCst),
            2,
            "the probe must run twice: once rejected, once with the refreshed credential"
        );
    }

    /// Issue #507: a connector with nothing to refresh (app password / API
    /// token) pauses as before, but the persisted outcome now carries the
    /// probe's actual rejection message instead of the generic "auth expired".
    #[tokio::test]
    async fn auth_expired_without_refresh_preserves_probe_message() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );
        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "no-refresh".to_string(),
                backend: "mock".to_string(),
                display_name: "No Refresh".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();
        let connector = Arc::new(ForceRefreshConnector {
            inner: MockConnector::from_config(json!({
                "__slug": "no-refresh",
                "mode": "polling",
                "interval_ms": 1,
                "jitter_ms": 0,
            }))
            .unwrap(),
            probes: AtomicU32::new(0),
            outcome: RefreshOutcome::Unchanged,
        });

        let outcome = run_cycle(
            connector,
            kg,
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        match outcome {
            CycleOutcome::AuthExpired(message) => assert_eq!(
                message, "IMAP auth rejected (BAD): invalid token",
                "the probe's rejection message must survive to the pause"
            ),
            _ => {
                panic!("expected AuthExpired with the probe message, got a non-AuthExpired outcome")
            }
        }
    }

    /// Issue #507: a failed forced refresh pauses with the provider's error
    /// (e.g. `invalid_grant`) so the persisted `last_error` names the actual
    /// cause instead of the generic "auth expired".
    #[tokio::test]
    async fn auth_expired_with_failed_refresh_pauses_with_refresh_error() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );
        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "refresh-fail".to_string(),
                backend: "mock".to_string(),
                display_name: "Refresh Fail".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();
        let connector = Arc::new(ForceRefreshConnector {
            inner: MockConnector::from_config(json!({
                "__slug": "refresh-fail",
                "mode": "polling",
                "interval_ms": 1,
                "jitter_ms": 0,
            }))
            .unwrap(),
            probes: AtomicU32::new(0),
            outcome: RefreshOutcome::Fail,
        });

        let outcome = run_cycle(
            connector,
            kg,
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        match outcome {
            CycleOutcome::AuthExpired(message) => assert_eq!(
                message,
                "authentication failed: token refresh failed: invalid_grant: refresh token revoked",
                "the refresh failure message must become the pause detail"
            ),
            _ => {
                panic!("expected AuthExpired with the refresh error, got a non-AuthExpired outcome")
            }
        }
    }

    /// Issue #507 review: a transient network failure during the forced
    /// refresh must not pause the connector — it is a recoverable cycle error
    /// (backoff + retry), so a later cycle can still recover once the network
    /// is back.
    #[tokio::test]
    async fn auth_expired_with_network_refresh_failure_retries_instead_of_pausing() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );
        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "refresh-net-fail".to_string(),
                backend: "mock".to_string(),
                display_name: "Refresh Net Fail".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();
        let connector = Arc::new(ForceRefreshConnector {
            inner: MockConnector::from_config(json!({
                "__slug": "refresh-net-fail",
                "mode": "polling",
                "interval_ms": 1,
                "jitter_ms": 0,
            }))
            .unwrap(),
            probes: AtomicU32::new(0),
            outcome: RefreshOutcome::NetworkFail,
        });

        let outcome = run_cycle(
            connector,
            kg,
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        match outcome {
            CycleOutcome::Err(message) => assert_eq!(
                message, "token refresh failed: connection refused",
                "a transient refresh failure must be a recoverable cycle error"
            ),
            _ => {
                panic!("expected a recoverable Err, got a non-Err outcome")
            }
        }
    }

    /// Delegating connector that reports a canned durable state after every
    /// extraction, simulating the Email connector's LLM-extraction retry
    /// ledger (issue #262).
    struct DurableStateConnector {
        inner: MockConnector,
        state: String,
        persisted: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Connector for DurableStateConnector {
        fn id(&self) -> &str {
            self.inner.id()
        }
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn connector_type(&self) -> ConnectorType {
            self.inner.connector_type()
        }
        fn mode(&self) -> ConnectorMode {
            self.inner.mode()
        }
        fn config_schema(&self) -> serde_json::Value {
            self.inner.config_schema()
        }
        async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
            self.inner.authenticate().await
        }
        async fn health(&self) -> Result<HealthStatus, ConnectorError> {
            self.inner.health().await
        }
        async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
            self.inner.sync(options).await
        }
        async fn extract(
            &self,
        ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
            self.inner.extract().await
        }
        async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
            self.inner.extract_deletions().await
        }
        async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
            self.inner.acknowledge_deletions(deleted).await
        }
        async fn act(
            &self,
            action: crate::connector::ConnectorAction,
        ) -> Result<crate::connector::ActionResult, ConnectorError> {
            self.inner.act(action).await
        }
        async fn forget(&self) -> Result<(), ConnectorError> {
            self.inner.forget().await
        }
        fn durable_state(&self) -> Option<String> {
            Some(self.state.clone())
        }
        fn durable_state_persisted(&self) {
            self.persisted.store(true, Ordering::Relaxed);
        }
    }

    /// A connector that reports durable state must have it persisted by the
    /// cycle (after extraction, alongside the cursor) so retries and
    /// terminal failures survive daemon restarts.
    #[tokio::test]
    async fn cycle_persists_connector_durable_state() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );

        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "durable-state".to_string(),
                backend: "mock".to_string(),
                display_name: "Durable State".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();

        let connector = Arc::new(DurableStateConnector {
            inner: MockConnector::from_config(json!({
                "__slug": "durable-state",
                "mode": "polling",
                "interval_ms": 1,
                "jitter_ms": 0,
            }))
            .unwrap(),
            state: "{\"pending\":{}}".to_string(),
            persisted: Arc::new(AtomicBool::new(false)),
        });

        let outcome = run_cycle(
            connector.clone(),
            kg.clone(),
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        assert!(matches!(outcome, CycleOutcome::Ok(_)));

        let persisted = kg.get_connector(row.id).await.unwrap().unwrap();
        assert_eq!(
            persisted.durable_state.as_deref(),
            Some("{\"pending\":{}}"),
            "durable state must be persisted by the cycle"
        );
        assert!(
            connector.persisted.load(Ordering::Relaxed),
            "the connector must acknowledge a successful persist"
        );
    }

    /// Delegating connector that fails the first `extract()` call and records
    /// every cursor adopted via `on_cycle_succeeded`, so a test can pin the
    /// supervisor contract: the connector is handed the new cursor only after
    /// a cycle fully succeeded (issue #314).
    struct FailFirstExtractCursorRecorder {
        inner: MockConnector,
        failed: AtomicBool,
        adopted: Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl Connector for FailFirstExtractCursorRecorder {
        fn id(&self) -> &str {
            self.inner.id()
        }
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn connector_type(&self) -> ConnectorType {
            self.inner.connector_type()
        }
        fn mode(&self) -> ConnectorMode {
            self.inner.mode()
        }
        fn config_schema(&self) -> serde_json::Value {
            self.inner.config_schema()
        }
        async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
            self.inner.authenticate().await
        }
        async fn health(&self) -> Result<HealthStatus, ConnectorError> {
            self.inner.health().await
        }
        async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
            self.inner.sync(options).await
        }
        async fn extract(
            &self,
        ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
            if !self.failed.swap(true, Ordering::SeqCst) {
                return Err(ConnectorError::Parse(
                    "injected transient extract failure".to_string(),
                ));
            }
            self.inner.extract().await
        }
        async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
            self.inner.extract_deletions().await
        }
        async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
            self.inner.acknowledge_deletions(deleted).await
        }
        async fn on_cycle_succeeded(&self, new_cursor: Option<&str>) {
            self.adopted
                .lock()
                .await
                .push(new_cursor.map(str::to_string));
        }
        async fn act(
            &self,
            action: crate::connector::ConnectorAction,
        ) -> Result<crate::connector::ActionResult, ConnectorError> {
            self.inner.act(action).await
        }
        async fn forget(&self) -> Result<(), ConnectorError> {
            self.inner.forget().await
        }
    }

    /// Issue #314: the supervisor hands the connector the new sync cursor
    /// only after a fully successful cycle. A cycle that fails after `sync`
    /// (extract error) must not advance the connector's in-memory cursor —
    /// the next in-process cycle then re-syncs from the last confirmed cursor
    /// and re-processes the failed window instead of skipping it.
    #[tokio::test]
    async fn cycle_adopts_new_cursor_only_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let kg = Arc::new(
            KnowledgeGraph::init(&dir.path().join("knowledge.db"))
                .await
                .unwrap(),
        );

        let row = kg
            .upsert_connector(UpsertConnectorInput {
                connector_type: ConnectorType::Email,
                slug: "cursor-adoption".to_string(),
                backend: "mock".to_string(),
                display_name: "Cursor Adoption".to_string(),
                config_json: "{}".to_string(),
                status: None,
                auth_state: None,
            })
            .await
            .unwrap();

        let connector = Arc::new(FailFirstExtractCursorRecorder {
            inner: MockConnector::from_config(json!({
                "__slug": "cursor-adoption",
                "facts": [
                    { "subject": "Alice", "relationship_type": "works_at", "object": "Acme" }
                ],
                "cursor": "tok-1",
            }))
            .unwrap(),
            failed: AtomicBool::new(false),
            adopted: Mutex::new(Vec::new()),
        });

        // Cycle 1: `sync` staged the fact, `extract` fails — the cursor must
        // NOT be adopted, so the next cycle re-syncs from the last confirmed
        // cursor.
        let first = run_cycle(
            connector.clone(),
            kg.clone(),
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        assert!(matches!(first, CycleOutcome::Err(_)));
        assert!(
            connector.adopted.lock().await.is_empty(),
            "a failed cycle must not adopt the new cursor"
        );

        // Cycle 2: succeeds — the supervisor must hand the persisted cursor
        // to the connector so the next cycle is incremental.
        let second = run_cycle(
            connector.clone(),
            kg.clone(),
            row.id,
            ConnectorType::Email,
            SyncOptions::default(),
        )
        .await;
        assert!(matches!(second, CycleOutcome::Ok(_)));
        assert_eq!(
            connector.adopted.lock().await.as_slice(),
            &[Some("tok-1".to_string())],
            "the connector adopts the cursor only after a successful cycle"
        );
    }
}
