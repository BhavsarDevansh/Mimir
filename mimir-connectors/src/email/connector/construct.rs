//! `EmailConnector` construction from configuration.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64};

use tokio::sync::Mutex;

use crate::connector::{ConnectorError, normalize_user_identity};
use crate::email::config::{
    DEFAULT_DISPLAY_NAME, DEFAULT_SLUG, EmailAuthMethod, EmailConfigDto, parse_cursor,
};
use crate::email::connector::EmailConnector;
use crate::email::imap;
use crate::email::llm::ProseRetryLedger;
use crate::oauth::OAuthHttpClient;
use crate::secrets::SecretStore;
use mimir_core::hooks::HookEngine;
use mimir_core::llm::LlmBackend;
use mimir_knowledge::KnowledgeGraph;
use tracing::warn;

/// Build a connector from its parsed configuration, a shared secret store
/// (optional), and the supervisor-injected cursor.
impl EmailConnector {
    pub fn from_config(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        cursor: Option<String>,
    ) -> Result<Self, ConnectorError> {
        Self::from_config_with_deps(config, secret_store, None, cursor, None, None, None)
    }

    /// Build a connector with optional injected dependencies: the canonical
    /// user identity, a shared LLM backend, the shared knowledge graph, and
    /// the shared hooks engine (tests inject mocks; the daemon passes the
    /// live services through the factory).
    pub fn from_config_with_deps(
        config: serde_json::Value,
        secret_store: Option<Arc<dyn SecretStore>>,
        user_identity: Option<String>,
        cursor: Option<String>,
        llm_backend: Option<Arc<dyn LlmBackend>>,
        kg: Option<Arc<KnowledgeGraph>>,
        hook_engine: Option<Arc<HookEngine>>,
    ) -> Result<Self, ConnectorError> {
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
        let dto: EmailConfigDto = serde_json::from_value(config)
            .map_err(|e| ConnectorError::Config(format!("invalid email config: {e}")))?;
        // Only an OAuth-configured connector needs the hardened OAuth client;
        // an app-password connector must not allocate a second reqwest
        // connection pool or fail startup if the OAuth client build fails.
        let oauth_http = match &dto.auth {
            EmailAuthMethod::OAuth { .. } => Some(OAuthHttpClient::new()?),
            EmailAuthMethod::AppPassword { .. } => None,
        };
        // Legacy migration (issue #386): pending retries persisted by the
        // pre-hooks engine re-stage into the buffer and are re-enqueued as
        // hooks on the next cycle. Entries without a decodable payload are
        // dropped (they were never persisted with one, or the payload is
        // corrupt).
        let pending_items = ledger.drain_legacy_pending();
        let mut buffer = Vec::with_capacity(pending_items.len());
        for pending in pending_items {
            match pending.raw() {
                Some(raw) => buffer.push(imap::RawEmail {
                    uid: pending.uid,
                    uid_validity: pending.uid_validity,
                    internal_date: None,
                    raw,
                }),
                None => {
                    warn!(
                        raw_ref = %pending.raw_ref(),
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
            supports_idle: StdMutex::new(None),
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
