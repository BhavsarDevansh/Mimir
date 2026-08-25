//! [`Connector`] trait implementation for the mock.

use std::sync::atomic::Ordering;

use chrono::Utc;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

use super::MockConnector;
use crate::connector::{
    ActionResult, Connector, ConnectorAction, ConnectorError, ConnectorMode, HealthStatus,
    SyncOptions, SyncOutcome,
};

#[async_trait::async_trait]
impl Connector for MockConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        self.ctype
    }

    fn mode(&self) -> ConnectorMode {
        self.mode_override
            .as_ref()
            .and_then(|mode| *mode.lock().unwrap())
            .unwrap_or(self.mode)
    }

    fn mode_if_resolved(&self) -> Option<ConnectorMode> {
        self.mode_resolution_override
            .as_ref()
            .map(|mode| *mode.lock().unwrap())
            .unwrap_or_else(|| Some(self.mode()))
    }

    fn config_schema(&self) -> serde_json::Value {
        Self::config_schema_value()
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        if self.auth_fail {
            return Err(ConnectorError::NotAuthenticated);
        }
        Ok(self.auth_state)
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        Ok(self.health.clone())
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let n = self.sync_calls.fetch_add(1, Ordering::SeqCst);

        // Track the complete sync() call. The guard is created before the
        // first await and dropped on return, panic unwind, or task
        // cancellation, so `in_flight` is always balanced and every call —
        // including injected failures and panics — is recorded.
        let _guard = self
            .recorder
            .as_ref()
            .map(|recorder| recorder.enter(options));

        // Push connectors block inside sync waiting for events; the mock
        // simulates this by sleeping the configured cadence. The supervisor
        // aborts the runner task on shutdown, cancelling the sleep.
        if matches!(self.mode, ConnectorMode::Push) {
            tokio::time::sleep(self.interval).await;
        }

        // Panic injection (counted as a failure by the supervisor).
        if n < self.panic_first {
            panic!("mock connector panic #{n}");
        }

        // Failure injection.
        if self.always_fail || n < self.fail_first {
            return Err(ConnectorError::Network(format!(
                "simulated mock failure #{n}"
            )));
        }

        // Optional artificial delay (serialization/concurrency tests). The
        // recorder guard above already brackets this, so overlapping triggers
        // are observable even if the delay is cancelled.
        if !self.sync_delay.is_zero() {
            tokio::time::sleep(self.sync_delay).await;
        }

        // Stage the canned facts for this cycle. With `batch_size`, slice
        // incrementally to completion; otherwise emit the full list. The batch
        // window is keyed on the *successful*-sync counter (`sync_successes`),
        // not the raw call counter (`n`), so failed/panicked cycles do not
        // consume a window and silently drop facts.
        let success_index = self.sync_successes.fetch_add(1, Ordering::SeqCst);
        let staged: Vec<NormalizedFact> = match self.batch_size {
            None => self
                .facts
                .iter()
                .enumerate()
                .map(|(i, f)| f.to_normalized(&self.slug, i))
                .collect(),
            Some(size) => {
                let size = size as usize;
                let start = (success_index as usize)
                    .saturating_mul(size)
                    .min(self.facts.len());
                let end = (start + size).min(self.facts.len());
                self.facts[start..end]
                    .iter()
                    .enumerate()
                    .map(|(offset, f)| f.to_normalized(&self.slug, start + offset))
                    .collect()
            }
        };

        let fetched = u32::try_from(staged.len()).unwrap_or(u32::MAX);
        self.buffer.lock().await.extend(staged);
        // Stage the configured deletions for this cycle (issue #247). Unlike
        // facts they are not batch-windowed: a mock server that keeps
        // re-reporting a tombstone until its cursor advances is the intended
        // shape, and the KB trash path is idempotent (re-reports are no-ops).
        self.tombstones.lock().await.extend(self.deletions.clone());

        Ok(SyncOutcome {
            fetched,
            new_cursor: self.cursor.clone(),
            fetched_at: Utc::now(),
        })
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        let mut buffer = self.buffer.lock().await;
        let drained = std::mem::take(&mut *buffer);
        Ok(drained)
    }

    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        let tombstones = self.tombstones.lock().await;
        // Non-destructive (PR #313 review): the supervisor acknowledges the
        // processed removals via `acknowledge_deletions` only after the
        // cycle's trashing, fact insertion, and cursor persistence all
        // succeeded, so a failed cycle re-reports them on the next cycle
        // instead of losing the tombstone.
        Ok(tombstones.clone())
    }

    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        let mut tombstones = self.tombstones.lock().await;
        tombstones.retain(|raw| !deleted.contains(raw));
        Ok(())
    }

    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError> {
        match self.act_kind.as_deref() {
            Some(kind) if kind == action.kind => {
                let native_id = action
                    .payload
                    .get("native_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let message = action
                    .payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                Ok(ActionResult {
                    success: true,
                    native_id,
                    message,
                })
            }
            _ => Err(ConnectorError::UnsupportedAction(action.kind)),
        }
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        // The mock holds no credentials or persisted local data; forget is a
        // no-op. The supervisor cascades KB facts via the trash machinery.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------
