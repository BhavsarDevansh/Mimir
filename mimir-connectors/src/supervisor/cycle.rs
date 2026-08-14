use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
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

// ---------------------------------------------------------------------------
// Runner task (one per active connector)
// ---------------------------------------------------------------------------

/// Outcome of a single sync cycle, returned from [`run_cycle`].
pub(super) enum CycleOutcome {
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
pub(super) enum CycleResult {
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
/// shutdown), run one cycle in an isolated sub-task (so a connector panic
/// does not kill the runner) with the chosen [`SyncOptions`], classify the
/// result, apply backoff / circuit-breaker / auth-expiry / shutdown policy,
/// and reply to any waiting trigger caller.
pub(super) async fn run_connector(
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
/// elapsing, a manual sync trigger, or shutdown. After a failed cycle the wait
/// is exponential backoff (still preemptable by a trigger) instead of the
/// polling interval.
///
/// Push-mode connectors loop immediately on success (they block inside `sync`
/// waiting for service events, so there is no polling interval to wait on);
/// manual triggers are rejected upstream for push connectors, so the trigger
/// channel is never selected in the push success arm.
pub(super) async fn wait_next(
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

    // Persist connector-side durable state (e.g. the Email connector's
    // bounded LLM-extraction retry ledger, issue #262) so retries and
    // terminal failures survive daemon restarts. `None` means "unchanged"
    // and skips the write.
    if let Some(state) = connector.durable_state() {
        if let Err(error) = kg.update_durable_state(instance_id, Some(&state)).await {
            return CycleOutcome::Err(error.to_string());
        }
    }

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

    use std::sync::atomic::{AtomicU32, Ordering};

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
                connector_type: ConnectorType::Gmail,
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
                connector_type: Some(ConnectorType::Gmail),
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
            ConnectorType::Gmail,
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
            ConnectorType::Gmail,
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

    /// Delegating connector that reports a canned durable state after every
    /// extraction, simulating the Email connector's LLM-extraction retry
    /// ledger (issue #262).
    struct DurableStateConnector {
        inner: MockConnector,
        state: String,
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
                connector_type: ConnectorType::Gmail,
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
        });

        let outcome = run_cycle(
            connector,
            kg.clone(),
            row.id,
            ConnectorType::Gmail,
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
    }
}
