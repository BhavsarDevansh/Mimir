//! `EmailConnector` construction from configuration.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use tokio::sync::Mutex;

use crate::connector::{ConnectorError, normalize_user_identity};
use crate::email::config::{
    DEFAULT_DISPLAY_NAME, DEFAULT_SLUG, EmailAuthMethod, EmailConfigDto, parse_cursor,
};
use crate::email::connector::EmailConnector;
use crate::email::llm::ProseRetryLedger;
use crate::oauth::OAuthHttpClient;
use crate::secrets::SecretStore;
use mimir_core::hooks::HookEngine;
use mimir_core::llm::LlmBackend;
use mimir_knowledge::KnowledgeGraph;
use tracing::warn;

/// Optional shared services injected into an [`EmailConnector`] at
/// construction. The daemon supplies the live services through the factory;
/// tests override only the fields they exercise. Named fields keep the
/// `Option`-heavy construction unambiguous (e.g. `user_identity` vs
/// `cursor`, which share the type `Option<String>`).
#[derive(Default)]
pub struct EmailConnectorDeps {
    pub secret_store: Option<Arc<dyn SecretStore>>,
    pub user_identity: Option<String>,
    pub cursor: Option<String>,
    pub llm_backend: Option<Arc<dyn LlmBackend>>,
    pub kg: Option<Arc<KnowledgeGraph>>,
    pub hook_engine: Option<Arc<HookEngine>>,
}

/// Build a connector from its parsed configuration, a shared secret store
/// (optional), and the supervisor-injected cursor.
impl EmailConnector {
    pub fn from_config(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        cursor: Option<String>,
    ) -> Result<Self, ConnectorError> {
        Self::from_config_with_deps(
            config,
            EmailConnectorDeps {
                secret_store,
                cursor,
                ..Default::default()
            },
        )
    }

    /// Build a connector with optional injected dependencies (tests inject
    /// mocks; the daemon passes the live services through the factory).
    pub fn from_config_with_deps(
        config: serde_json::Value,
        deps: EmailConnectorDeps,
    ) -> Result<Self, ConnectorError> {
        let EmailConnectorDeps {
            secret_store,
            user_identity,
            cursor,
            llm_backend,
            kg,
            hook_engine,
        } = deps;
        // Recover the supervisor-injected slug before parsing the DTO: serde
        // ignores unknown fields (the DTO has no `deny_unknown_fields`), so
        // the injected `__slug` / `__cursor` keys pass through harmlessly.
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());
        // Recover the supervisor-injected instance id for hook provenance.
        let instance_id = config
            .get("__instance_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        // Seed the durable connector state (issues #262, #283) from the
        // supervisor-injected `__durable_state` (the `connectors.durable_state`
        // column persisted after the previous cycle): the bounded
        // LLM-extraction retry ledger (re-staging the pending raw RFC 822
        // bytes into the buffer, so a restart resumes the retry without an
        // IMAP re-fetch — the cursor has advanced past the message) and the
        // buffered iMIP `CANCEL` tombstones (so a restart between `extract`
        // and the supervisor's deletion pass re-reports the removals). A
        // pending entry whose payload is missing or undecodable (never
        // persisted because it exceeded the size cap, or corrupt) is settled
        // (dropped) rather than left to linger.
        let mut ledger = config
            .get("__durable_state")
            .and_then(|v| v.as_str())
            .map(ProseRetryLedger::from_json)
            .unwrap_or_default();
        // Seed the cached IMAP `IDLE` capability from the persisted durable
        // state (issue #397 review): a fresh instance — a daemon restart or a
        // `resolved_mode` construction — resolves `Auto` mode without a live
        // probe once the previous cycle persisted the capability.
        let supports_idle = ledger.supports_idle();
        let dto: EmailConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid email config: {e}")))?;
        // Only an OAuth-configured connector needs the hardened OAuth client;
        // an app-password connector must not allocate a second reqwest
        // connection pool or fail startup if the OAuth client build fails.
        let oauth_http = match &dto.auth {
            EmailAuthMethod::OAuth { .. } => Some(OAuthHttpClient::new()?),
            EmailAuthMethod::AppPassword { .. } => None,
        };
        // Re-stage pending retries — legacy pre-hooks entries (issue #386)
        // and queue-overflow entries written when the hook's pending queue
        // was full (issue #442 review) — into the buffer; the next cycle
        // re-enqueues them as hooks. Entries without a decodable payload are
        // dropped (they were never persisted with one, or the payload is
        // corrupt).
        let pending_items = ledger.drain_pending();
        let mut buffer = Vec::with_capacity(pending_items.len());
        for pending in pending_items {
            let raw_ref = pending.raw_ref();
            match pending.into_staged_item() {
                Some(mail) => buffer.push(mail),
                None => {
                    warn!(
                        raw_ref = %raw_ref,
                        "dropping legacy pending prose retry with missing or undecodable payload"
                    );
                }
            }
        }
        Ok(Self {
            slug,
            display_name: dto
                .display_name
                .clone()
                .unwrap_or_else(|| DEFAULT_DISPLAY_NAME.to_string()),
            config: dto,
            secret_store,
            oauth_http,
            last_uid: Mutex::new(cursor.as_deref().and_then(parse_cursor)),
            resync_pending: AtomicBool::new(false),
            consecutive_connection_lost: AtomicU32::new(0),
            supports_idle: StdMutex::new(supports_idle),
            buffer: Mutex::new(buffer),
            prose_retry: Arc::new(StdMutex::new(ledger)),
            user_identity: normalize_user_identity(user_identity),
            llm_backend,
            kg,
            hook_engine,
            instance_id,
            durable_snapshot_version: AtomicU64::new(0),
        })
    }
}
