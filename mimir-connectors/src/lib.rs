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
//! - `mock` — configurable mock connector test harness (F13 / #190): emits
//!   canned `NormalizedFact`s on a configurable cadence in both `Polling` and
//!   `Push` modes, with health/auth/failure/panic injection, and is the T1
//!   sync→extract→insert→query vehicle. Includes `MockFactConfig` (canned-fact
//!   DTO) and `MockSyncRecorder` (sync-options observation for concurrency
//!   tests). Test-only, gated by the `test-mock-connector` feature (off by
//!   default).
//! - [`rate_limit`] — shared rate-limiting + retry/backoff primitives
//!   ([`RateLimitConfig`] / [`RateLimiter`] / [`BackoffStrategy`] / [`retry_with_backoff`],
//!   F12 / #189): token-bucket throttling (governor GCRA), optional rolling 24h
//!   daily quota, and uniform 429/503 retry with jitter. Connector LLM calls are
//!   exempt (decision D′); this governs HTTP/IMAP/CalDAV API calls only.
//! - `oauth` — shared OAuth 2.0 client + token-refresh helpers (issue #240),
//!   used by the Calendar (C3 / #197) and Email (C5 / #199) OAuth connectors
//!   and (from A4 / #205) the CLI PKCE login. Built on `oauth2` 5.0.0 with
//!   `default-features = false` and a custom `oauth::OAuthHttpClient` adapter
//!   over the workspace's single reqwest 0.13 client — the crate's optional
//!   reqwest 0.12 dependency never enters the tree. Gated by the `oauth`
//!   feature (enabled by `calendar`, `gmail`, and the CLI).
//! - [`secrets`] — [`SecretStore`] trait + [`SecretBundle`] enum +
//!   [`FileSecretStore`] / [`InMemorySecretStore`] (F10 / #187) +
//!   `KeyringSecretStore` (F11 / #188, opt-in `secrets-keyring`): per-connector
//!   credential storage, one store for all auth kinds (OAuth / API token / app
//!   password). V1 default is file-backed, plaintext at rest, 0600/0700 perms.
//! - [`supervisor`] — [`ConnectorSupervisor`] + [`SupervisorConfig`]
//!   (F8 / #185): supervised per-connector task lifecycle (spawn / restart /
//!   backoff / circuit-breaker / startup-restore / graceful-shutdown /
//!   cursor-persistence). Also owns manual sync triggering (F9 / #186):
//!   [`ConnectorSupervisor::trigger_sync`] preempts a connector's polling
//!   interval with caller-supplied [`SyncOptions`] and serialises concurrent
//!   triggers via a per-connector semaphore, returning the cycle outcome.
//! - `mock_oauth` — in-process mock OAuth 2.0 authorization server (T2 /
//!   #207): an HTTPS `/authorize` + HTTP `/token` loopback pair that the PKCE
//!   flow E2E tests drive without a real provider. Test-only, gated by the
//!   `test-mock-oauth` feature (off by default).
//!
//! # Feature flags
//!
//! `photos`, `calendar`, and `gmail` gate the per-type backends, which are
//! added in later Phase 3 issues (C1–C7); `oauth` is the shared OAuth 2.0
//! client + refresh layer enabled by `calendar`/`gmail` and the CLI PKCE flow.
//! `test-mock-oauth` gates the in-process mock OAuth server used by the T2
//! E2E tests and `test-mock-connector` gates the configurable mock connector
//! harness (F13 / #190); both are test-only and off by default, and
//! `test-utils` gates the shared OAuth test doubles (issues #290, #298).
//! The framework core is **always built**; the mock connector is a test
//! harness gated by `test-mock-connector` (off by default), so
//! `--no-default-features` still compiles a working framework without the
//! harness.

pub mod connector;
#[cfg(any(feature = "photos", feature = "calendar", feature = "gmail", test))]
mod fact;
pub mod geocoder;
/// Shared iCalendar VEVENT parsing + fact extraction (Phase 3 C4 / #198
/// and C6 / #200). Needed by both the Calendar and Email backends that
/// consume iCalendar data; gated by `any(feature = "calendar", feature = "gmail")`.
#[cfg(any(feature = "calendar", feature = "gmail"))]
pub mod ical;
/// Configurable mock connector test harness (Phase 3 F13 / #190): an
/// in-memory connector whose behaviour is driven entirely by `config_json`,
/// with health/auth/failure/panic injection and sync-options observation.
/// Test-only, gated by the `test-mock-connector` feature (off by default);
/// the crate's own unit tests compile the module via `cfg(test)` regardless,
/// and downstream crates opt in through dev-dependencies.
#[cfg(any(feature = "test-mock-connector", test))]
pub mod mock;
/// In-process mock OAuth 2.0 authorization server for tests (Phase 3 T2 /
/// #207): an HTTPS `/authorize` + HTTP `/token` loopback pair that the PKCE
/// flow E2E tests drive without a real provider. Gated by the
/// `test-mock-oauth` feature (off by default; enabled by this crate's
/// integration tests and the `mimir` binary's daemon-level tests).
#[cfg(feature = "test-mock-oauth")]
pub mod mock_oauth;
/// OAuth 2.0 client + token-refresh helpers (issue #240), gated by the `oauth`
/// feature. [`oauth::OAuthHttpClient`] implements the `oauth2` crate's
/// `AsyncHttpClient` trait over the workspace reqwest 0.13 client; the
/// refresh helpers (`refresh_token`, `resolve_access_token`) drive the
/// vetted `oauth2` 5.0.0 refresh grant with the workspace's secret-hygiene
/// error mapping (parsed `error`/`error_description` only, never the raw
/// response body) and HTTPS/loopback endpoint gate.
#[cfg(feature = "oauth")]
pub mod oauth;
pub mod rate_limit;
pub mod registry;
pub mod secrets;
pub mod supervisor;
/// Shared OAuth test doubles (Phase 3 / issues #290, #298): a fake-browser
/// opener, authorize-URL parsing, and the wiremock token-endpoint mock used
/// by the PKCE flow's unit tests and the `mimir` binary's CLI connector
/// tests. Test-only, gated by the `test-utils` feature (off by default);
/// also compiled for this crate's own unit tests via `cfg(test)`.
#[cfg(any(feature = "test-utils", test))]
pub mod test_utils;

/// Local-filesystem Photos connector (Phase 3 C1 / #195), gated by the
/// `photos` feature.
#[cfg(feature = "photos")]
pub mod photos;

/// CalDAV calendar connector (Phase 3 C3 / #197), gated by the `calendar`
/// feature. A `CalDavClient` (PROPFIND + sync-collection REPORT, sync-token
/// incremental sync, icalendar VEVENT parsing) backs a `CalendarConnector`
/// implementing the two-step ingestion model in `Polling` mode.
#[cfg(feature = "calendar")]
pub mod calendar;

/// IMAP email connector (Phase 3 C5 / #199), gated by the `gmail` feature. An
/// [`email::imap`] transport (IMAP `LOGIN` / `AUTHENTICATE XOAUTH2`, `UID
/// FETCH` incremental sync, `IDLE` push) backs an [`EmailConnector`] running
/// in `Push` (IDLE) or `Polling` (fallback) mode.
#[cfg(feature = "gmail")]
pub mod email;

pub use connector::{
    ActionResult, Connector, ConnectorAction, ConnectorContext, ConnectorError, ConnectorFactory,
    ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
pub use geocoder::{DEFAULT_NOMINATIM_ENDPOINT, NominatimConfig, NominatimGeocoder};
#[cfg(any(feature = "test-mock-connector", test))]
pub use mock::{
    MockConnector, MockConnectorFactory, MockFactConfig, MockSyncGuard, MockSyncRecorder,
};
pub use registry::{ConnectorRegistry, FnConnectorFactory};
pub use supervisor::{
    ActError, ConnectorSupervisor, SupervisorConfig, SupervisorError, TriggerError, TriggerOutcome,
};

#[cfg(feature = "photos")]
pub use photos::{PhotosConnector, PhotosConnectorFactory, PhotosCursor};

#[cfg(feature = "calendar")]
pub use calendar::{
    CalendarAuthMethod, CalendarConfigDto, CalendarConnector, CalendarConnectorFactory,
};

#[cfg(feature = "gmail")]
pub use email::{
    EmailAuthMethod, EmailConfigDto, EmailConnector, EmailConnectorFactory, EmailSyncMode,
};
pub use rate_limit::{
    BackoffStrategy, QuotaSnapshot, RateLimitConfig, RateLimitError, RateLimiter, RetryError,
    RetryHint, Retryable, is_retryable_status, retry_with_backoff,
};
#[cfg(all(
    feature = "secrets-keyring",
    any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "macos",
        target_os = "windows"
    )
))]
pub use secrets::KeyringSecretStore;
pub use secrets::{FileSecretStore, InMemorySecretStore, SecretBundle, SecretError, SecretStore};
