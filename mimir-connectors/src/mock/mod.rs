//! Configurable mock connector — the framework's test harness (Phase 3 F13 /
//! issue #190).
//!
//! `MockConnector` is an in-memory connector whose behaviour is driven
//! entirely by its `config_json`: it emits canned [`mimir_knowledge::normalize::NormalizedFact`]s on a
//! configurable cadence, in either [`ConnectorMode::Polling`] or
//! [`ConnectorMode::Push`], and can inject failures, panics, and health/auth
//! states to exercise the [`ConnectorSupervisor`](crate::supervisor). It is
//! Test-only, gated by the `test-mock-connector` feature (off by default) so
//! production builds never compile the harness; the crate's own unit tests
//! compile it via `cfg(test)` regardless. It is the vehicle for the T1
//! sync→extract→insert→query end-to-end test without real services.
//!
//! # Two-step ingestion model
//!
//! [`Connector::sync`](crate::connector::Connector::sync) stages the configured facts into an internal buffer and
//! returns a [`SyncOutcome`](crate::connector::SyncOutcome) (item count + cursor); [`Connector::extract`](crate::connector::Connector::extract)
//! drains that buffer into `Vec<NormalizedFact>`. The supervisor then inserts
//! them through [`mimir_knowledge::normalize::normalize_and_insert`]. The mock
//! never touches the database.
//!
//! # Instance identity
//!
//! The supervisor injects `__slug`, `__ctype`, and `__instance_id` into a
//! connector's `config_json` before handing it to the factory (see
//! [`crate::supervisor`]). [`MockConnector::from_config`] reads these to
//! recover its identity. When they are absent (e.g. a bare
//! [`MockConnector::default`]) it falls back to the legacy no-op identity
//! (`id "mock"`, type `Email`).
//!
//! # Push mode
//!
//! Push connectors block inside [`Connector::sync`](crate::connector::Connector::sync) waiting for service events.
//! The mock simulates this by sleeping the configured `interval_ms` at the
//! start of every `sync()` (the "schedule"), then staging the canned facts.
//! The supervisor aborts the runner task on shutdown, cancelling the in-flight
//! sleep; F9 manual triggers are rejected for push connectors, so no trigger
//! path is needed.

mod config;
mod connector;
mod factory;
mod recorder;
mod sync_impl;

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;
use tokio::sync::Mutex;

use crate::connector::{ConnectorMode, HealthStatus};
use config::{DEFAULT_INTERVAL_MS, DEFAULT_JITTER_MS, default_display_name, default_slug};

pub use config::MockFactConfig;
pub use factory::MockConnectorFactory;
pub use recorder::{MockSyncGuard, MockSyncRecorder};

// ---------------------------------------------------------------------------
// Connector struct + identity defaults
// ---------------------------------------------------------------------------
pub struct MockConnector {
    slug: String,
    display_name: String,
    ctype: ConnectorType,
    mode: ConnectorMode,
    /// Runtime override consulted by [`Connector::mode`](crate::connector::Connector::mode)
    /// before the configured `mode` (issue #397 review). Lets a test flip a
    /// running connector between push and polling — e.g. to prove a manual
    /// sync trigger consults the *live* mode (an `auto`-mode email connector
    /// resolves to polling once its capability probe completes) rather than a
    /// spawn-time snapshot. Shared via `Arc` so the test and the supervisor's
    /// cloned instance observe the same value.
    mode_override: Option<Arc<StdMutex<Option<ConnectorMode>>>>,
    /// Runtime override consulted by
    /// [`Connector::mode_if_resolved`](crate::connector::Connector::mode_if_resolved)
    /// before the default `Some(self.mode())` (issue #475): while present,
    /// the trait method reports the wrapped value verbatim — `None`
    /// simulates an unprobed `auto` connector whose capability probe has not
    /// completed yet, `Some(mode)` pins the resolved mode. Shared via `Arc`
    /// so the test and the supervisor's cloned instance observe the same
    /// value.
    mode_resolution_override: Option<Arc<StdMutex<Option<ConnectorMode>>>>,
    facts: Vec<MockFactConfig>,
    batch_size: Option<u32>,
    health: HealthStatus,
    auth_state: ConnectorAuthState,
    fail_first: u32,
    panic_first: u32,
    always_fail: bool,
    auth_fail: bool,
    cursor: Option<String>,
    sync_delay: Duration,
    interval: Duration,
    recorder: Option<Arc<MockSyncRecorder>>,
    sync_calls: AtomicU32,
    /// Counts only *successful* syncs; drives the `batch_size` slice so
    /// failed/panicked cycles do not consume a batch window.
    sync_successes: AtomicU32,
    buffer: Mutex<Vec<NormalizedFact>>,
    /// Raw references to report as server-side deletions via
    /// `extract_deletions` (issue #247), staged into `tombstones` by `sync`.
    deletions: Vec<String>,
    /// Staged tombstone raw references awaiting `extract_deletions`; kept
    /// until the supervisor acknowledges them via `acknowledge_deletions`
    /// (PR #313 review) so a failed cycle re-reports them.
    tombstones: Mutex<Vec<String>>,
    /// When set, `act()` accepts this action kind and returns a canned
    /// [`ActionResult`] echoing the payload's `native_id` / `message` (Phase 3
    /// A2 / #203). Any other kind yields `UnsupportedAction`.
    act_kind: Option<String>,
}

impl Default for MockConnector {
    fn default() -> Self {
        Self {
            slug: default_slug(),
            display_name: default_display_name(),
            ctype: ConnectorType::Email,
            mode: ConnectorMode::Polling {
                interval: Duration::from_millis(DEFAULT_INTERVAL_MS),
                jitter: Duration::from_millis(DEFAULT_JITTER_MS),
            },
            mode_override: None,
            mode_resolution_override: None,
            facts: Vec::new(),
            batch_size: None,
            health: HealthStatus::Online,
            auth_state: ConnectorAuthState::Authenticated,
            fail_first: 0,
            panic_first: 0,
            always_fail: false,
            auth_fail: false,
            cursor: None,
            sync_delay: Duration::ZERO,
            interval: Duration::from_millis(DEFAULT_INTERVAL_MS),
            recorder: None,
            sync_calls: AtomicU32::new(0),
            sync_successes: AtomicU32::new(0),
            buffer: Mutex::new(Vec::new()),
            deletions: Vec::new(),
            tombstones: Mutex::new(Vec::new()),
            act_kind: None,
        }
    }
}

impl std::fmt::Debug for MockConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockConnector")
            .field("slug", &self.slug)
            .field("ctype", &self.ctype)
            .field("mode", &self.mode)
            .field("facts", &self.facts.len())
            .field("batch_size", &self.batch_size)
            .field("health", &self.health)
            .field("fail_first", &self.fail_first)
            .field("panic_first", &self.panic_first)
            .field("always_fail", &self.always_fail)
            .field("auth_fail", &self.auth_fail)
            .field("cursor", &self.cursor)
            .finish()
    }
}
