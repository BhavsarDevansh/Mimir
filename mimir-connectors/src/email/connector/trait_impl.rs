//! [`Connector`] trait implementation for the email backend.

use std::time::Duration;

use async_trait::async_trait;
use mail_parser::MessageParser;
use tracing::{debug, warn};

use crate::connector::{
    Connector, ConnectorError, ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
use crate::email::config::{EmailSyncMode, parse_cursor};
use crate::email::connector::EmailConnector;
use crate::email::imap;
use crate::email::llm::{FailureDisposition, RetryGate, health_with_terminal};
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
                            "required": ["kind", "username", "auth_uri", "token_endpoint", "client_id"],
                            "properties": {
                                "kind": { "const": "oauth" },
                                "username": { "type": "string" },
                                "auth_uri": { "type": "string", "format": "uri" },
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
                "llm_extraction_max_attempts": { "type": "integer", "minimum": 1, "maximum": 255, "default": 3 },
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
        let probe = match self.probe_capability().await {
            Ok(_) => HealthStatus::Online,
            Err(ConnectorError::NotAuthenticated) => HealthStatus::NotConfigured,
            Err(ConnectorError::Authentication(_)) => HealthStatus::AuthExpired,
            Err(ConnectorError::Network(_)) => HealthStatus::Offline,
            Err(e) => return Err(e),
        };
        Ok(health_with_terminal(
            probe,
            self.prose_retry.lock().unwrap().terminal_count(),
        ))
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let session = self.open_session().await?;
        self.run_sync(session, options).await
    }

    async fn on_cycle_succeeded(&self, new_cursor: Option<&str>) {
        // Adopt the persisted cursor as the in-memory progress marker only
        // now that the supervisor confirmed the whole cycle succeeded (issue
        // #332, mirroring #314). Advancing inside `run_sync` would skip the
        // failed cycle's staged mail on the next in-process cycle: the
        // persisted cursor is only updated on a fully successful cycle, so
        // the in-memory marker must never run ahead of it. `None` means
        // "cursor unchanged" and leaves the marker as-is.
        if let Some(cursor) = new_cursor {
            match parse_cursor(cursor) {
                Some(parsed) => *self.last_uid.lock().await = Some(parsed),
                None => warn!(cursor, "ignoring unparseable email cursor from supervisor"),
            }
        }
    }

    async fn extract(
        &self,
    ) -> Result<Vec<mimir_knowledge::normalize::NormalizedFact>, ConnectorError> {
        // C6 / #200 + #249: drain the staged RFC 822 messages and run a
        // deterministic (structured-parse) extraction *cascade* over each. The
        // cascade has two layers today:
        //   1. iMIP calendar invites (`text/calendar; method=REQUEST | REPLY
        //      | CANCEL`) parsed into the same VEVENT fact cluster the
        //      Calendar connector emits, via the shared [`crate::ical`]
        //      module (C6 / #200). REQUEST/REPLY facts are keyed by the
        //      VEVENT UID; a CANCEL emits no facts and stages its UID as a
        //      tombstone (issue #283).
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
            // Durable retry ledger (issue #262): a message awaiting a bounded
            // LLM retry skips this cycle during its backoff window (staying
            // staged); a message whose retry budget is exhausted is dropped
            // without re-processing, so it stops consuming LLM calls.
            let gate = self.prose_retry.lock().unwrap().gate(&raw_ref);
            match gate {
                RetryGate::Backoff => {
                    self.buffer.lock().await.push(mail.clone());
                    continue;
                }
                RetryGate::SkippedTerminal => {
                    debug!(raw_ref, "skipping permanently-failed LLM extraction");
                    continue;
                }
                RetryGate::Attempt => {}
            }
            if let Some(message) = MessageParser::default().parse(&mail.raw) {
                // Layers 1-2: deterministic extraction (structured parse). Both
                // run on the same parsed Message so there is no second MIME
                // parse, and both tag their facts with
                // `extraction_method = StructuredParse`.
                let before = facts.len();
                let imip_handled = self.extract_invites(&message, &raw_ref, &mut facts);
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
                // Only run it when layers 1-2 produced *no* facts and no iMIP
                // part was handled for this message (a CANCEL emits no facts,
                // and a REQUEST/REPLY whose VEVENT failed to parse emits none
                // either), so a deterministic layer already read the email
                // (machine-readable invite / JSON-LD) is never re-processed
                // by the LLM (avoids duplicate extraction and bounds LLM
                // cost). When no backend is injected the layer is skipped,
                // leaving deterministic extraction unchanged.
                if facts.len() != before || imip_handled {
                    // A deterministic layer read the message (facts, or an
                    // iMIP lifecycle signal like a CANCEL that emits none);
                    // settle any stale retry entry so it cannot resurrect a
                    // retry, and skip the LLM layer so cancellation prose
                    // cannot author junk facts.
                    self.prose_retry.lock().unwrap().settle(&raw_ref);
                } else if let Some(backend) = &self.llm_backend {
                    match llm::extract_prose_facts(
                        backend,
                        self.user_identity.as_deref(),
                        &message,
                        &raw_ref,
                    )
                    .await
                    {
                        Ok(prose_facts) => {
                            // The message was read (facts, or an explicit
                            // no-facts verdict); settle the ledger.
                            self.prose_retry.lock().unwrap().settle(&raw_ref);
                            facts.extend(prose_facts);
                        }
                        // A retryable LLM failure must not become a silent
                        // empty extraction: the buffer was drained and the
                        // IMAP cursor advanced, so re-staging the raw email
                        // keeps it for the next extraction cycle. The retry
                        // is bounded (issue #262): the ledger counts attempts,
                        // waits an exponential cycle backoff, and records a
                        // terminal failure once the budget is exhausted.
                        // Deterministic facts already collected this cycle
                        // are kept, so a transient LLM error never blocks
                        // them.
                        Err(error) => {
                            let max_attempts =
                                self.config.llm_extraction_max_attempts.clamp(1, u8::MAX);
                            let disposition = self.prose_retry.lock().unwrap().record_failure(
                                &raw_ref,
                                mail.uid_validity,
                                mail.uid,
                                &mail.raw,
                                max_attempts,
                                error.to_string(),
                            );
                            match disposition {
                                FailureDisposition::Retry { skip_cycles } => {
                                    warn!(
                                        raw_ref,
                                        skip_cycles,
                                        "LLM email extraction failed; re-staging raw email for bounded retry: {error}"
                                    );
                                    self.buffer.lock().await.push(mail.clone());
                                }
                                FailureDisposition::Terminal => {
                                    warn!(
                                        raw_ref,
                                        max_attempts,
                                        "LLM email extraction permanently failed; skipping message: {error}"
                                    );
                                }
                            }
                        }
                    }
                } else {
                    // No backend configured: nothing to extract and no retry
                    // is possible — settle any stale ledger entry so it
                    // cannot linger across restarts.
                    self.prose_retry.lock().unwrap().settle(&raw_ref);
                }
            } else {
                debug!(uid = mail.uid, "could not parse RFC 822 message; skipping");
            }
        }
        // Issue #283: a CANCEL must win over a same-batch REQUEST regardless
        // of message order. `extract_invites` buffers each CANCEL's
        // namespaced reference as a tombstone; the supervisor trashes
        // *before* inserting this cycle's facts, so any fact whose
        // `raw_reference` matches a pending tombstone must be dropped here
        // or the cancelled event would be inserted after the trash and
        // survive. Filtering once after the message loop (not only at CANCEL
        // time) covers a CANCEL staged before its REQUEST in the same batch:
        // buffer order is not guaranteed to match iMIP order (re-staged LLM
        // retries are pushed to the back of the buffer, and mail delivery
        // can invert order). The `imip:` namespace keeps the filter from
        // ever touching JSON-LD / LLM facts, whose references live in the
        // `{uid_validity}:{uid}` space.
        let ledger = self.prose_retry.lock().unwrap();
        let tombstones = ledger.tombstones();
        if !tombstones.is_empty() {
            facts.retain(|f| {
                !f.raw_reference
                    .as_deref()
                    .is_some_and(|r| tombstones.iter().any(|t| t.as_str() == r))
            });
        }
        Ok(facts)
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        self.buffer.lock().await.clear();
        self.prose_retry.lock().unwrap().clear();
        *self.last_uid.lock().await = None;
        if let Some(store) = &self.secret_store {
            store.delete(&self.slug).await.map_err(|e| {
                ConnectorError::Authentication(format!("secret delete failed: {e}"))
            })?;
        }
        Ok(())
    }

    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        // Issue #283: report the buffered iMIP CANCEL references without
        // draining them — the supervisor acknowledges the processed
        // removals via `acknowledge_deletions` only after trashing, fact
        // insertion, and cursor persistence all succeeded, so a failed cycle
        // re-reports them instead of losing them (the #247 retention
        // contract). Each reference is the namespaced `raw_reference` the
        // iMIP layer authors for the cancelled event's facts, so the
        // supervisor trashes exactly those facts. The buffer is part of the
        // durable state, so a restart between `extract` and the deletion
        // pass re-reports the removals instead of losing them.
        let ledger = self.prose_retry.lock().unwrap();
        Ok(ledger.tombstones().to_vec())
    }

    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        self.prose_retry
            .lock()
            .unwrap()
            .acknowledge_deletions(deleted);
        Ok(())
    }

    fn durable_state(&self) -> Option<String> {
        self.prose_retry.lock().unwrap().durable_json()
    }

    fn durable_state_persisted(&self) {
        self.prose_retry.lock().unwrap().mark_persisted();
    }
}
