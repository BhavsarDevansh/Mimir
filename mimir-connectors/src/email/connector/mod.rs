//! IMAP email connector (Phase 3 C5 / #199): sync, extract, and auth flows.
//!
//! Construction lives in [`construct`], credential handling in
//! [`credentials`], IMAP session/sync in [`session`], invite extraction in
//! [`extract`], and the [`Connector`] trait adapter in [`trait_impl`].

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use tokio::sync::Mutex;

use crate::email::config::EmailConfigDto;
use crate::email::imap;
use crate::oauth::OAuthHttpClient;
use crate::secrets::SecretStore;
use mimir_core::llm::LlmBackend;

mod construct;
mod credentials;
mod extract;
#[cfg(test)]
#[path = "../extract_tests.rs"]
mod extract_tests;
#[cfg(test)]
#[path = "../imap_tests.rs"]
mod imap_tests;
#[cfg(test)]
#[path = "../kb_tests.rs"]
mod kb_tests;
#[cfg(test)]
#[path = "../llm_tests.rs"]
mod llm_tests;
mod session;
mod trait_impl;

#[cfg(test)]
use crate::connector::{Connector, ConnectorError, ConnectorMode, SyncOptions};
#[cfg(test)]
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};

pub struct EmailConnector {
    slug: String,
    display_name: String,
    config: EmailConfigDto,
    /// Shared credential store (loaded by slug); `None` means the daemon did
    /// not wire one in (sync/authenticate then fail `NotAuthenticated`).
    secret_store: Option<Arc<dyn SecretStore>>,
    /// OAuth HTTP client for token refresh (issue #240): the `oauth2`-crate
    /// adapter over the workspace reqwest 0.13 client, built with redirects
    /// disabled so a credential POST can never be bounced to another host.
    /// `None` for non-OAuth auth methods — the client is only built when the
    /// config actually needs it (an app-password connector must not allocate
    /// a second connection pool or fail startup on an OAuth client build
    /// error).
    oauth_http: Option<OAuthHttpClient>,
    /// In-memory incremental cursor (`(uid_validity, last_uid)`). Seeded from
    /// `__cursor` at construction; advanced only via
    /// [`Connector::on_cycle_succeeded`] after the supervisor persists the
    /// reported cursor on a fully successful cycle (issue #332), so a failed
    /// cycle re-syncs from the last confirmed cursor.
    pub(crate) last_uid: Mutex<Option<(u32, u32)>>,
    /// `true` when the last `sync()` reported a moved cursor that the
    /// supervisor has not yet confirmed via [`Connector::on_cycle_succeeded`]
    /// (issue #332). The next `sync()` then skips the IDLE wait and re-fetches
    /// from the last confirmed cursor, because the IDLE notification for the
    /// failed window will not re-fire — a failed cycle's staged mail would
    /// otherwise sit unprocessed until the next push.
    pub(crate) resync_pending: AtomicBool,
    /// Cached `IDLE` capability, set by [`authenticate`](Connector::authenticate).
    /// `None` until the first successful capability probe. A
    /// `std::sync::Mutex` (never held across an `await`) so the
    /// sync [`mode`](Connector::mode) can read it without `try_lock`.
    pub(crate) supports_idle: StdMutex<Option<bool>>,
    /// Staged raw RFC 822 messages awaiting extraction (drained by `extract`).
    buffer: Mutex<Vec<imap::RawEmail>>,
    /// Durable connector state (issues #262, #283): the bounded
    /// LLM-extraction retry ledger (pending retries with attempt counts +
    /// exponential cycle backoff, and terminal failures with reasons) plus
    /// the buffered iMIP `CANCEL` tombstones awaiting the supervisor's
    /// deletion pass. Persisted by the supervisor via
    /// [`Connector::durable_state`](crate::connector::Connector::durable_state)
    /// and re-injected at construction (`__durable_state`), so bounded
    /// retries and pending cancellations survive daemon restarts. A
    /// `std::sync::Mutex` (never held across an `await`).
    prose_retry: StdMutex<crate::email::llm::ProseRetryLedger>,
    /// Canonical user identity name (the `config.toml` `[identity] name`),
    /// injected via [`ConnectorContext::user_identity`] so connector-sourced
    /// facts that are user-scoped — iMIP invite extraction's
    /// `user has_event <event>` (C6 / #200) — author against the same entity
    /// the daemon treats as `user_entity_id`. `None` when no identity is
    /// configured (the primary `has_event` fact is then skipped, matching the
    /// Calendar connector's behaviour).
    user_identity: Option<String>,
    /// Shared LLM backend for the prose-extraction layer (C7 / #201).
    /// `None` when the daemon injected no backend; the LLM layer is
    /// then skipped (deterministic layers 1-2 still run). Calls route
    /// through [`LlmBackend::system_chat_message`] so they sit on the
    /// shared pool's system queue, below user-chat priority.
    llm_backend: Option<Arc<dyn LlmBackend>>,
}
