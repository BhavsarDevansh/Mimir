use tokio::sync::oneshot;

use crate::connector::SyncOptions;
use mimir_knowledge::models::enums::ConnectorStatus;

// ---------------------------------------------------------------------------
// Manual sync trigger types (F9 / #186)
// ---------------------------------------------------------------------------

/// A manual sync request queued from [`ConnectorSupervisor::trigger_sync`](super::runner::ConnectorSupervisor::trigger_sync) to
/// a connector's runner task. Carries the caller's [`SyncOptions`] and a
/// [`oneshot::Sender`] to deliver the cycle's outcome back to the caller.
pub(super) struct TriggerRequest {
    pub(super) options: SyncOptions,
    pub(super) reply: oneshot::Sender<TriggerOutcome>,
}

/// Capacity of the per-connector trigger channel.
///
/// The per-connector [`Semaphore`] (one permit) is held across the send and
/// the reply await, so at most one trigger request is ever in flight per
/// connector — a previous request is always drained and replied before the
/// next caller is allowed to send. A capacity of one therefore never blocks
/// the sender and is the smallest sufficient buffer.
pub(super) const TRIGGER_CHANNEL_CAPACITY: usize = 1;

/// Outcome of a manually-triggered sync cycle, returned to the caller of
/// [`ConnectorSupervisor::trigger_sync`](super::runner::ConnectorSupervisor::trigger_sync).
///
/// Mirrors the runner's internal `CycleOutcome`: a successful cycle reports
/// the connector's [`SyncOutcome`](crate::connector::SyncOutcome) stats; `AuthExpired` reports that the
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
    /// The service reported expired, revoked, or rejected auth; the connector
    /// has been paused. Carries the underlying auth rejection message
    /// (issue #507).
    AuthExpired(String),
    /// The cycle failed with a recoverable error.
    Failed(String),
}

/// Errors raised by [`ConnectorSupervisor::trigger_sync`](super::runner::ConnectorSupervisor::trigger_sync) /
/// [`ConnectorSupervisor::trigger_sync_by_slug`](super::runner::ConnectorSupervisor::trigger_sync_by_slug).
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
    /// The connector's mode is *resolved* to push. Manual sync triggers
    /// preempt the polling interval, which push-mode connectors do not have;
    /// push-mode manual sync is deferred to a later Phase 3 issue. An `auto`
    /// connector whose capability probe has not completed yet is not rejected
    /// (issue #475).
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
