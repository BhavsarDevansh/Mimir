//! [`Connector`] trait implementation for the email backend.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use mail_parser::MessageParser;
use tracing::{debug, warn};

use crate::connector::{
    Connector, ConnectorError, ConnectorMode, CredentialRefresh, HealthStatus, SyncOptions,
    SyncOutcome,
};
use crate::email::config::{EmailSyncMode, parse_cursor};
use crate::email::connector::EmailConnector;
use crate::email::envelope::EmailEnvelope;
use crate::email::imap;
use crate::email::jsonld;
use crate::email::llm::{EmailExtractionPayload, health_with_terminal};
use crate::secrets::SecretBundle;
use mimir_core::hooks::{Trigger, TriggerStatus};
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};

#[async_trait]
impl CredentialRefresh for EmailConnector {
    fn secret_store(&self) -> Option<Arc<dyn crate::secrets::SecretStore>> {
        self.secret_store.clone()
    }

    fn connector_slug(&self) -> &str {
        &self.slug
    }

    async fn forced_refresh(
        &self,
        bundle: &SecretBundle,
    ) -> Result<Option<SecretBundle>, ConnectorError> {
        self.resolve_auth(bundle, true)
            .await
            .map(|(_, refreshed)| refreshed)
    }

    async fn persist_refreshed_bundle(&self, bundle: &SecretBundle) -> Result<(), ConnectorError> {
        self.persist_refreshed(bundle).await
    }
}

#[async_trait]
impl Connector for EmailConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Email
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

    fn mode_if_resolved(&self) -> Option<ConnectorMode> {
        match self.config.mode {
            // `Auto` needs the capability probe to pick between Push and
            // Polling; a fresh instance (no cached capability yet) cannot
            // resolve the mode, so the caller omits it instead of claiming
            // Push before the probe completes (issue #397 review).
            EmailSyncMode::Auto if self.supports_idle.lock().unwrap().is_none() => None,
            _ => Some(self.mode()),
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
                "initial_backfill": { "type": "boolean", "default": true },
                "idle_timeout_secs": { "type": "integer", "default": 1500 },
                "connect_timeout_secs": { "type": "integer", "default": 10 },
                "handshake_timeout_secs": { "type": "integer", "default": 30 },
                "read_timeout_secs": { "type": "integer", "default": 60 },
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
            Err(ConnectorError::Authentication(message)) => HealthStatus::AuthExpired(message),
            Err(ConnectorError::Network(_)) => HealthStatus::Offline,
            Err(e) => return Err(e),
        };
        Ok(health_with_terminal(
            probe,
            self.prose_retry.lock().unwrap().terminal_count(),
        ))
    }

    async fn force_refresh(&self) -> Result<ConnectorAuthState, ConnectorError> {
        self.force_refresh_credentials().await
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
        // The pending re-sync flag is deliberately NOT cleared here: it is
        // cleared by the next successful fetch inside `run_sync`, so a cycle
        // that returned without fetching (an IDLE connection dropped
        // mid-window) is followed by a re-fetch cycle before IDLE resumes.
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
            // Re-stage durable queue-overflow payloads (issue #442 review):
            // a message whose `connector_item.remember` enqueue was rejected
            // when the hook's pending queue was full is recorded in the
            // ledger with its raw bytes (and re-staged at construction after
            // a restart); drain them now so this cycle re-attempts the
            // enqueue instead of waiting for a restart. Legacy pre-hooks
            // pending retries take the same path (issue #386).
            // Dedupe against the current buffer (issue #332 mirror): a
            // failed-cycle re-fetch or a `--full` re-sync can stage the same
            // message the overflow entry carries, and `QueuePolicy::Multiple`
            // would happily enqueue both copies.
            let mut seen: HashSet<(u32, u32)> =
                buffer.iter().map(|m| (m.uid_validity, m.uid)).collect();
            for pending in self.prose_retry.lock().unwrap().drain_pending() {
                let raw_ref = pending.raw_ref();
                match pending.into_staged_item() {
                    Some(mail) => {
                        if seen.insert((mail.uid_validity, mail.uid)) {
                            buffer.push(mail);
                        }
                    }
                    None => {
                        warn!(
                            raw_ref = %raw_ref,
                            "dropping pending prose retry with missing or undecodable payload"
                        );
                    }
                }
            }
            std::mem::take(&mut *buffer)
        };
        let mut facts = Vec::new();
        // The mailbox address is per-instance and constant across the batch;
        // derive it once so the envelope and every hook payload share the
        // same value without re-reading the config per message.
        let mailbox_address = self.mailbox_address();
        for mail in staged {
            // An IMAP UID is unique only within one mailbox + `UIDVALIDITY`
            // epoch, so qualify the provenance reference as `{uid_validity}:{uid}`
            // (matching the persisted cursor format) to stay globally unique.
            let raw_ref = format!("{}:{}", mail.uid_validity, mail.uid);
            // Terminal-failure ledger (issues #262, #386): a message whose
            // retry budget was exhausted is dropped without re-processing,
            // so it stops consuming LLM calls.
            if self.prose_retry.lock().unwrap().is_terminal(&raw_ref) {
                debug!(raw_ref, "skipping permanently-failed LLM extraction");
                continue;
            }
            if let Some(message) = MessageParser::default().parse(&mail.raw) {
                // Issue #398: derive the message envelope once — dates,
                // sender, recipients, and the deterministic spam /
                // forwarding / misdirection signals — and gate the whole
                // cascade on it. Bulk mail is skipped before any layer, so
                // a marketing broadcast carrying an iMIP invite or JSON-LD
                // can no longer author facts.
                let envelope = EmailEnvelope::from_message(
                    &message,
                    mail.internal_date,
                    mailbox_address.as_deref(),
                );
                if envelope.is_spam {
                    debug!(raw_ref, "skipping extraction cascade: bulk-marketing email");
                    continue;
                }
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
                // Only enqueue it when layers 1-2 produced *no* facts and no
                // iMIP part was handled for this message (a CANCEL emits no
                // facts, and a REQUEST/REPLY whose VEVENT failed to parse
                // emits none either), so a deterministic layer already read
                // the email (machine-readable invite / JSON-LD) is never
                // re-processed by the LLM (avoids duplicate extraction and
                // bounds LLM cost). The `connector_item.remember` hook
                // runner owns retry with time-based backoff (issue #386);
                // terminal failures are recorded durably in the shared
                // ledger by the hook handler. When no backend or hook engine
                // is injected the layer is skipped, leaving deterministic
                // extraction unchanged.
                if facts.len() != before || imip_handled {
                    // A deterministic layer read the message (facts, or an
                    // iMIP lifecycle signal like a CANCEL that emits none);
                    // skip the LLM layer so cancellation prose cannot author
                    // junk facts.
                } else if let (Some(engine), Some(backend), Some(kg)) =
                    (&self.hook_engine, &self.llm_backend, &self.kg)
                {
                    let payload = EmailExtractionPayload {
                        // The payload owns a copy of the raw RFC 822 bytes;
                        // the staged item itself stays alive for the duration
                        // of the trigger call so a full-queue rejection can
                        // persist the bytes as a durable overflow instead of
                        // dropping the message (issue #442 review). The copy
                        // is transient: on success the staged item is dropped
                        // and only the hook payload retains the bytes.
                        raw: mail.raw.clone(),
                        internal_date: mail.internal_date,
                        mailbox_address: mailbox_address.clone(),
                        uid_validity: mail.uid_validity,
                        uid: mail.uid,
                        raw_ref: raw_ref.clone(),
                        user_identity: self.user_identity.clone(),
                        instance_id: self.instance_id,
                        connector_type: self.connector_type(),
                        kg: Arc::clone(kg),
                        llm: Arc::clone(backend),
                        ledger: Arc::clone(&self.prose_retry),
                        max_attempts: self.config.llm_extraction_max_attempts.clamp(1, u8::MAX),
                    };
                    let outcomes = engine
                        .trigger(Trigger::ConnectorItemStaged {
                            item_id: raw_ref.clone(),
                            payload: Arc::new(payload),
                        })
                        .await;
                    if outcomes
                        .iter()
                        .any(|o| o.status == TriggerStatus::QueueFull)
                    {
                        warn!(
                            %raw_ref,
                            "connector_item.remember pending queue full; re-staging email as durable overflow"
                        );
                        // The hook's pending queue is full, so the email
                        // cannot be enqueued this cycle. Record it durably
                        // (raw bytes base64-encoded, bounded) so the next
                        // cycle — or a restart — re-stages and retries it;
                        // otherwise `extract` would return Ok, the supervisor
                        // would advance the IMAP cursor, and the message
                        // would never be fetched again.
                        self.prose_retry.lock().unwrap().record_overflow(
                            raw_ref.clone(),
                            mail.uid_validity,
                            mail.uid,
                            mail.internal_date,
                            mail.raw,
                        );
                    }
                } else {
                    // No backend or hook engine configured: nothing to
                    // extract.
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
        self.resync_pending.store(false, Ordering::SeqCst);
        self.consecutive_connection_lost.store(0, Ordering::SeqCst);
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
        let (version, json) = self.prose_retry.lock().unwrap().durable_json()?;
        self.durable_snapshot_version
            .store(version, Ordering::Relaxed);
        Some(json)
    }

    fn durable_state_persisted(&self) {
        let version = self.durable_snapshot_version.load(Ordering::Relaxed);
        self.prose_retry.lock().unwrap().mark_persisted(version);
    }
}
