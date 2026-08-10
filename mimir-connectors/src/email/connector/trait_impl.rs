//! [`Connector`] trait implementation for the email backend.

use std::time::Duration;

use async_trait::async_trait;
use mail_parser::MessageParser;
use tracing::{debug, warn};

use crate::connector::{
    Connector, ConnectorError, ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
use crate::email::config::EmailSyncMode;
use crate::email::connector::EmailConnector;
use crate::email::imap;
use crate::email::{jsonld, llm};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};

#[async_trait]
impl Connector for EmailConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Gmail
    }

    fn mode(&self) -> ConnectorMode {
        match self.config.mode {
            EmailSyncMode::Idle => ConnectorMode::Push,
            EmailSyncMode::Poll => ConnectorMode::Polling {
                interval: Duration::from_secs(self.config.poll_interval_secs),
                jitter: Duration::from_secs(self.config.poll_jitter_secs),
            },
            EmailSyncMode::Auto => match *self.supports_idle.lock().unwrap() {
                // `mode()` is a sync method called after `authenticate()`, so
                // the cached capability is set; `None` (not yet probed) defaults
                // to Push — IDLE is preferred and ubiquitous for the targeted
                // providers (Gmail / Outlook / iCloud).
                Some(false) => ConnectorMode::Polling {
                    interval: Duration::from_secs(self.config.poll_interval_secs),
                    jitter: Duration::from_secs(self.config.poll_jitter_secs),
                },
                _ => ConnectorMode::Push,
            },
        }
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["host", "auth"],
            "properties": {
                "host": { "type": "string" },
                "port": { "type": "integer", "default": 993 },
                "mailbox": { "type": "string", "default": "INBOX" },
                "auth": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["kind", "username"],
                            "properties": {
                                "kind": { "const": "app_password" },
                                "username": { "type": "string" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["kind", "username", "token_endpoint", "client_id"],
                            "properties": {
                                "kind": { "const": "oauth" },
                                "username": { "type": "string" },
                                "token_endpoint": { "type": "string", "format": "uri" },
                                "client_id": { "type": "string" },
                                "client_secret": { "type": "string" },
                                "scopes": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    ]
                },
                "mode": { "type": "string", "enum": ["auto", "idle", "poll"], "default": "auto" },
                "poll_interval_secs": { "type": "integer", "default": 300 },
                "poll_jitter_secs": { "type": "integer", "default": 30 },
                "idle_timeout_secs": { "type": "integer", "default": 1680 },
                "display_name": { "type": "string" }
            }
        })
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        match self.probe_capability().await {
            Ok(_) => Ok(ConnectorAuthState::Authenticated),
            Err(ConnectorError::NotAuthenticated) => Ok(ConnectorAuthState::Unauthenticated),
            Err(ConnectorError::Authentication(_)) => Ok(ConnectorAuthState::Expired),
            Err(e) => Err(e),
        }
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        match self.probe_capability().await {
            Ok(_) => Ok(HealthStatus::Online),
            Err(ConnectorError::NotAuthenticated) => Ok(HealthStatus::NotConfigured),
            Err(ConnectorError::Authentication(_)) => Ok(HealthStatus::AuthExpired),
            Err(ConnectorError::Network(_)) => Ok(HealthStatus::Offline),
            Err(e) => Err(e),
        }
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let session = self.open_session().await?;
        self.run_sync(session, options).await
    }

    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        // C6 / #200 + #249: drain the staged RFC 822 messages and run a
        // deterministic (structured-parse) extraction *cascade* over each. The
        // cascade has two layers today:
        //   1. iMIP calendar invites (`text/calendar; method=REQUEST | REPLY`)
        //      parsed into the same VEVENT fact cluster the Calendar connector
        //      emits, via the shared [`crate::ical`] module (C6 / #200).
        //   2. schema.org JSON-LD (`<script type="application/ld+json">` in
        //      HTML parts) parsed into typed fact clusters for Order,
        //      ParcelDelivery, FlightReservation, LodgingReservation,
        //      EventReservation, Ticket, and ReservationPackage (#249).
        // Plain prose emails (no `text/calendar` part and no JSON-LD) produce
        // no facts: the email is *provenance*, not the fact — the fact is about
        // the real-world thing the email conveys, and unstructured prose
        // confirmations, flights, and bank statements that carry no
        // machine-readable JSON-LD are read by the LLM layer in C7 / #201. No
        // per-email communication facts are emitted and no `Person` entities
        // are auto-created from `From`/`To` headers, so marketing/spam produces
        // no junk facts.
        // Drain the staged buffer and release the mutex guard before the
        // CPU-bound MIME parse loop: holding the lock across parsing would block
        // a concurrent `sync()` cycle from staging new mail for the whole parse.
        let staged: Vec<imap::RawEmail> = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };
        let mut facts = Vec::new();
        for mail in &staged {
            // An IMAP UID is unique only within one mailbox + `UIDVALIDITY`
            // epoch, so qualify the provenance reference as `{uid_validity}:{uid}`
            // (matching the persisted cursor format) to stay globally unique.
            let raw_ref = format!("{}:{}", mail.uid_validity, mail.uid);
            if let Some(message) = MessageParser::default().parse(&mail.raw) {
                // Layers 1-2: deterministic extraction (structured parse). Both
                // run on the same parsed Message so there is no second MIME
                // parse, and both tag their facts with
                // `extraction_method = StructuredParse`.
                let before = facts.len();
                facts.extend(self.extract_invites(&message, &raw_ref));
                // Layer 2: schema.org JSON-LD deterministic extraction
                // (#249). Scans HTML parts for <script type="application/ld+json">
                // and emits typed facts for recognised schema.org types
                // (Order, ParcelDelivery, FlightReservation, …). No LLM —
                // pure Rust parsing.
                //
                // Known limitation: when a single email carries both an iMIP
                // invite and an equivalent JSON-LD `EventReservation` for the
                // same booking, both layers fire and the graph gains two
                // `Event` entities / appointment overlays (the layers derive
                // the event name from different fields, so
                // `normalize_and_insert` does not dedupe them). Reconciling
                // overlapping facts across cascade layers is tracked as
                // follow-up work.
                facts.extend(jsonld::extract_facts_from_message(
                    self.user_identity.as_deref(),
                    &message,
                    &raw_ref,
                ));

                // Layer 3: LLM extraction (C7 / #201) — the last-resort layer
                // for unstructured prose a deterministic layer cannot read.
                // Only run it when layers 1-2 produced *no* facts for this
                // message, so a deterministic layer already read the email
                // (machine-readable invite / JSON-LD) is never re-processed by
                // the LLM (avoids duplicate extraction and bounds LLM cost).
                // When no backend is injected the layer is skipped, leaving
                // deterministic extraction unchanged.
                if facts.len() == before {
                    if let Some(backend) = &self.llm_backend {
                        match llm::extract_prose_facts(
                            backend,
                            self.user_identity.as_deref(),
                            &message,
                            &raw_ref,
                        )
                        .await
                        {
                            Ok(prose_facts) => facts.extend(prose_facts),
                            // A retryable LLM failure must not become a
                            // silent empty extraction: the buffer was drained
                            // and the IMAP cursor advanced, so re-staging the
                            // raw email keeps it for the next extraction cycle
                            // (until extraction succeeds or a durable retry /
                            // terminal-failure policy lands). Deterministic
                            // facts already collected this cycle are kept, so
                            // a transient LLM error never blocks them.
                            Err(error) => {
                                warn!(
                                    raw_ref,
                                    "LLM email extraction failed; re-staging raw email for retry: {error}"
                                );
                                self.buffer.lock().await.push(mail.clone());
                            }
                        }
                    }
                }
            } else {
                debug!(uid = mail.uid, "could not parse RFC 822 message; skipping");
            }
        }
        Ok(facts)
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        self.buffer.lock().await.clear();
        *self.last_uid.lock().await = None;
        if let Some(store) = &self.secret_store {
            store.delete(&self.slug).await.map_err(|e| {
                ConnectorError::Authentication(format!("secret delete failed: {e}"))
            })?;
        }
        Ok(())
    }
}
