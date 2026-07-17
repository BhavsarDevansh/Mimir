#![deny(unsafe_code)]
//! `mimir-connectors` — service ingestion framework for Mimir.
//!
//! Connectors are background sync workers that fetch data from external
//! services (email, calendar, photos, …), normalize it, and insert it into the
//! knowledge graph through the *existing* [`mimir_knowledge`] fact pipeline.
//! They are not a parallel track: every connector funnels through the same
//! `normalize_and_insert` boundary as conversational `remember` calls.
//!
//! # Database access boundary
//!
//! DB access is mediated **exclusively** by the
//! [`mimir_knowledge::KnowledgeGraph`] facade. This crate never holds a
//! `sqlx` pool handle directly, and does not depend on `sqlx` itself.
//!
//! # Ingestion model
//!
//! [`Connector::sync`] fetches raw items into a connector-internal buffer;
//! [`Connector::extract`] drains them into `Vec<NormalizedFact>`. The
//! supervisor (F8) then calls `mimir_knowledge::normalize::normalize_and_insert`
//! to resolve entities, score confidence, gate sensitivity, and insert. The
//! connector itself never touches the database.
//!
//! # Crate layout
//!
//! - [`connector`] — runtime [`Connector`] trait + data types
//!   (`ConnectorMode`, `SyncOptions`, `SyncOutcome`, `HealthStatus`,
//!   `ConnectorAction`, `ActionResult`, `ConnectorError`) and the
//!   [`ConnectorFactory`] trait.
//! - [`registry`] — [`ConnectorRegistry`] and multi-backend factory dispatch
//!   (F7 / #184): maps `(connector_type, backend)` to a [`ConnectorFactory`]
//!   and constructs instances on demand; includes the closure-backed
//!   [`FnConnectorFactory`].
//! - [`mock`] — always-compiled mock connector test harness (stub; filled by
//!   F13).
//! - [`rate_limit`] — shared rate-limiting + retry/backoff primitives
//!   ([`RateLimitConfig`] / [`RateLimiter`] / [`BackoffStrategy`] / [`retry_with_backoff`],
//!   F12 / #189): token-bucket throttling (governor GCRA), optional rolling 24h
//!   daily quota, and uniform 429/503 retry with jitter. Connector LLM calls are
//!   exempt (decision D′); this governs HTTP/IMAP/CalDAV API calls only.
//! - [`secrets`] — [`SecretStore`] trait + [`SecretBundle`] enum +
//!   [`FileSecretStore`] / [`InMemorySecretStore`] (F10 / #187): per-connector
//!   credential storage, one store for all auth kinds (OAuth / API token / app
//!   password). V1 default is file-backed, plaintext at rest, 0600/0700 perms.
//! - [`supervisor`] — [`ConnectorSupervisor`] + [`SupervisorConfig`]
//!   (F8 / #185): supervised per-connector task lifecycle (spawn / restart /
//!   backoff / circuit-breaker / startup-restore / graceful-shutdown /
//!   cursor-persistence). Also owns manual sync triggering (F9 / #186):
//!   [`ConnectorSupervisor::trigger_sync`] preempts a connector's polling
//!   interval with caller-supplied [`SyncOptions`] and serialises concurrent
//!   triggers via a per-connector semaphore, returning the cycle outcome.
//!
//! # Feature flags
//!
//! `photos`, `calendar`, and `gmail` gate the per-type backends, which are
//! added in later Phase 3 issues (C1–C7). The framework core and the mock
//! connector are **always built**, so `--no-default-features` still compiles a
//! working framework + mock harness.

pub mod connector;
pub mod mock;
pub mod rate_limit;
pub mod registry;
pub mod secrets;
pub mod supervisor;

pub use connector::{
    ActionResult, Connector, ConnectorAction, ConnectorError, ConnectorFactory, ConnectorMode,
    HealthStatus, SyncOptions, SyncOutcome,
};
pub use mock::{MockConnector, MockConnectorFactory};
pub use registry::{ConnectorRegistry, FnConnectorFactory};
pub use supervisor::{
    ConnectorSupervisor, SupervisorConfig, SupervisorError, TriggerError, TriggerOutcome,
};

pub use rate_limit::{
    BackoffStrategy, QuotaSnapshot, RateLimitConfig, RateLimitError, RateLimiter, RetryError,
    RetryHint, Retryable, is_retryable_status, retry_with_backoff,
};
pub use secrets::{FileSecretStore, InMemorySecretStore, SecretBundle, SecretError, SecretStore};
