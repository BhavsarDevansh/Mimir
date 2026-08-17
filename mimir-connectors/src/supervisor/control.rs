use std::time::Duration;

use tokio::sync::oneshot;

use mimir_knowledge::models::enums::ConnectorStatus;

use crate::connector::{ActionResult, ConnectorAction, ConnectorMode, SyncOptions};

use super::error::{ActError, SupervisorError};
use super::runner::{ConnectorHandle, ConnectorSupervisor};
use super::trigger::{TriggerError, TriggerOutcome, TriggerRequest};

/// How long `shutdown` waits for a runner to exit gracefully on its stop
/// signal before aborting it as a straggler.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl ConnectorSupervisor {
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

    /// Gracefully stop every runner: signal each runner, await its exit, then
    /// abort any straggler and await its in-flight cycle.
    ///
    /// The per-runner stop signal is sent first so each runner aborts and
    /// awaits its in-flight cycle sub-task before exiting — a runner only
    /// returns once its cycle task is gone (issue #266). Runners still alive
    /// after `SHUTDOWN_GRACE` are aborted as a defensive fallback for
    /// stragglers; their cycle handles remain registered in the shared
    /// `CycleRegistry`, which is drained and awaited afterwards, so no cycle
    /// task can outlive `shutdown` even on the abort path.
    pub async fn shutdown(&self) {
        let handles: Vec<ConnectorHandle> =
            self.handles.lock().await.drain().map(|(_, h)| h).collect();
        // Signal every runner to stop (the graceful path: the runner aborts
        // and awaits its in-flight cycle before exiting).
        for handle in &handles {
            let _ = handle.stop_tx.send(true);
        }
        // Await the graceful exits, bounded by the grace period so a
        // straggler cannot stall daemon shutdown forever. `is_finished()` is
        // polled rather than awaiting the `JoinHandle` here: the handle is
        // awaited once below, and a second poll of a completed handle panics.
        for handle in &handles {
            let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
            while !handle.task.is_finished() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        // Defensive fallback: abort any runner that did not exit, then await
        // its termination.
        for handle in &handles {
            if !handle.task.is_finished() {
                handle.task.abort();
            }
        }
        for handle in handles {
            let _ = handle.task.await;
        }
        // Abort + await any cycle whose runner was aborted mid-cycle: the
        // registry retained the handle, so no cycle task outlives `shutdown`
        // (issue #266).
        let cycles = self.cycle_tasks.drain().await;
        for cycle in &cycles {
            cycle.abort();
        }
        for cycle in cycles {
            // Await only handles that have not already completed: a handle
            // polled to completion by the runner's select must not be polled
            // again (tokio panics on that), and a completed cycle needs no
            // await — its task is already gone.
            if !cycle.is_finished() {
                let _ = cycle.await;
            }
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
    /// Signals the runner to stop and awaits its termination: the runner
    /// aborts and awaits its in-flight cycle before exiting, so no cycle
    /// outlives `stop` (issue #266 — a detached cycle would keep syncing and
    /// overlap the next runner's first cycle after a re-spawn). A runner
    /// still in its auth handshake exits immediately too, so `stop` never
    /// waits on a slow or hung network handshake. The `ConnectorHandle` is
    /// dropped so a subsequent [`restore`](Self::restore) or
    /// [`trigger_sync`](Self::trigger_sync) will treat the instance as down.
    /// The connector row is **not** deleted here — row lifecycle is the
    /// daemon's responsibility; this only manages the in-memory task.
    /// Persisting the current sync cursor happens in the runner's normal
    /// shutdown path; a stopped mid-cycle cycle is treated the same as
    /// `mimir stop` (the cursor reflects the last *completed* sync).
    ///
    /// Returns `true` if a runner was stopped, `false` if no live runner exists
    /// for `id` (already finished, never spawned, or previously stopped).
    pub async fn stop(&self, id: i32) -> bool {
        let handle = self.handles.lock().await.remove(&id);
        match handle {
            // Live runner: signal it to stop and await its termination (the
            // runner aborts and awaits its in-flight cycle before exiting),
            // then report that a runner was stopped.
            Some(handle) if !handle.task.is_finished() => {
                let _ = handle.stop_tx.send(true);
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
    /// [`resume`](Self::resume) and (future) reconfig-on-restart. The
    /// per-connector lifecycle lock is held across the whole
    /// stop → instantiate → spawn sequence, so concurrent `start` / `resume`
    /// calls for the same instance serialise instead of racing (issue #266).
    ///
    /// # Errors
    ///
    /// - [`SupervisorError::Knowledge`] — the row lookup or status write
    ///   failed, or no row matches `id` ([`KnowledgeError::ConnectorNotFound`](mimir_knowledge::KnowledgeError::ConnectorNotFound)).
    /// - [`SupervisorError::UnknownConnectorType`] — the row's
    ///   `connector_type_id` is not a known [`ConnectorType`](mimir_knowledge::models::enums::ConnectorType).
    /// - [`SupervisorError::Connector`] / [`SupervisorError::Json`] — the
    ///   row's `config_json` could not be parsed or the factory rejected it.
    pub async fn start(&self, id: i32) -> Result<(), SupervisorError> {
        // Serialise against a concurrent pause / resume and the daemon's
        // forget cascade for the same instance: the lock is held for the
        // whole stop → instantiate → spawn sequence, so a re-spawn is
        // idempotent under concurrency (issue #266) and can never sync
        // against a row that is about to be deleted.
        let _guard = self.lifecycle_lock(id).await;

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
    /// to `Paused` so the persisted state reflects the request. The
    /// per-connector lifecycle lock is held across the whole
    /// stop → status-write sequence, so a concurrent `start` / `resume` can
    /// never leave a `Paused` row with a live runner (issue #266).
    pub async fn pause(&self, id: i32) -> Result<(), SupervisorError> {
        // Serialise against a concurrent start/resume (and the daemon's
        // forget cascade) for the same instance: the lifecycle lock is held
        // across the whole stop → status-write sequence so a concurrent
        // re-spawn can never leave a `Paused` row with a live runner
        // (issue #266).
        let _guard = self.lifecycle_lock(id).await;
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
    /// Uses the live, running connector instance only when its runner is
    /// still alive (checked via `task.is_finished()`, matching
    /// [`trigger_sync`](Self::trigger_sync)). A handle left behind by a
    /// runner that exited naturally (auth-expiry, circuit-breaker, or panic)
    /// is dropped and treated as "no live instance" — its in-memory connector
    /// may hold expired credentials, so re-instantiating from the row reads
    /// fresh credentials from the [`SecretStore`](crate::secrets::SecretStore). When no live runner exists
    /// (a `Paused` / `Setup` / `Error` connector, or one whose runner exited),
    /// the connector is re-instantiated from its row — backends like the
    /// Calendar connector re-read credentials from the [`SecretStore`](crate::secrets::SecretStore) inside
    /// `act`, so they do not depend on the runner's auth handshake. The
    /// connector's own [`ConnectorError`](crate::connector::ConnectorError) (e.g.
    /// [`ConnectorError::UnsupportedAction`](crate::connector::ConnectorError::UnsupportedAction)) is returned for the server to
    /// map onto an HTTP status.
    pub async fn act(&self, id: i32, action: ConnectorAction) -> Result<ActionResult, ActError> {
        let connector = match self.live_connector(id).await {
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
                self.instantiate(&row, connector_type)?
            }
        };
        Ok(connector.act(action).await?)
    }

    /// Run a connector's local `forget()` cleanup (Phase 3 A2 / #203).
    ///
    /// Stops the runner (if any) and invokes [`Connector::forget`](crate::connector::Connector::forget) on the
    /// live instance when its runner is still alive, or on a freshly
    /// re-instantiated instance otherwise — mirroring [`act`](Self::act). The
    /// knowledge-graph fact trash and row deletion are the daemon's
    /// responsibility; this method only handles the connector-local cleanup
    /// half of the cascade. The caller must hold the per-connector
    /// [`lifecycle_lock`](Self::lifecycle_lock) for `id` for the whole
    /// cascade (the daemon's forget route does), so a concurrent
    /// [`start`](Self::start) / [`resume`](Self::resume) / [`pause`](Self::pause)
    /// cannot re-spawn the runner mid-cleanup.
    ///
    /// # Errors
    ///
    /// - [`SupervisorError::Knowledge`] — the row lookup failed, or no row
    ///   matches `id` ([`KnowledgeError::ConnectorNotFound`](mimir_knowledge::KnowledgeError::ConnectorNotFound)).
    /// - [`SupervisorError::UnknownConnectorType`] — the row's
    ///   `connector_type_id` is not a known [`ConnectorType`](mimir_knowledge::models::enums::ConnectorType).
    /// - [`SupervisorError::Connector`] / [`SupervisorError::Json`] — the
    ///   row's `config_json` could not be parsed, the factory rejected it, or
    ///   the connector's own `forget()` failed.
    pub async fn forget(&self, id: i32) -> Result<(), SupervisorError> {
        // Capture the live connector BEFORE stopping the runner: `stop()`
        // removes the handle and the aborted runner task drops its `Arc`, so
        // the live instance's in-memory state (e.g. the Photos watcher) is
        // exactly what `forget()` must tear down. The clone keeps the
        // instance alive across the stop.
        let live = self.live_connector(id).await;

        // Stop the runner before forget() so a mid-cycle sync cannot write
        // back to a vanishing row or race the connector-local cleanup.
        self.stop(id).await;

        let connector = match live {
            Some(c) => c,
            None => {
                let row = self
                    .kg
                    .get_connector(id)
                    .await?
                    .ok_or(SupervisorError::Knowledge(
                        mimir_knowledge::KnowledgeError::ConnectorNotFound(id),
                    ))?;
                let connector_type =
                    row.connector_type()
                        .ok_or(SupervisorError::UnknownConnectorType {
                            id,
                            type_id: row.connector_type_id,
                        })?;
                self.instantiate(&row, connector_type)?
            }
        };
        connector.forget().await?;
        Ok(())
    }
}
