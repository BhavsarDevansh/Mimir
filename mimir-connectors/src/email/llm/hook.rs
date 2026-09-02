//! The `connector_item.remember` hook handler (issue #386): runs the prose
//! extraction layer for one staged email and inserts the facts through the
//! shared pipeline with connector provenance.
//!
//! The hooks engine owns queueing and retry (time-based exponential
//! backoff); this handler only extracts and inserts. Terminal failures are
//! recorded durably in the connector's shared [`ProseRetryLedger`] so the
//! message is never re-processed and the failure surfaces via `Degraded`
//! health.

#![deny(unsafe_code)]

use std::any::Any;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use mail_parser::MessageParser;
use tracing::{debug, warn};

use mimir_core::hooks::{HookContext, HookHandler, HookOutcome};
use mimir_core::llm::LlmBackend;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::enums::ConnectorType;
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::{Provenance, normalize_and_insert};

use super::extract_prose_facts;
use super::retry::ProseRetryLedger;

/// Payload for one `ConnectorItemStaged` instance: a prose email awaiting
/// LLM extraction, plus everything the handler needs to extract and insert
/// without touching the connector instance (which may be dropped while the
/// hook is pending).
pub(crate) struct EmailExtractionPayload {
    /// Raw RFC 822 bytes.
    pub raw: Vec<u8>,
    /// IMAP `INTERNALDATE` (server receive time) of the staged message.
    /// Not part of the RFC 5322 headers, so it travels with the payload to
    /// survive the hook boundary (issue #398).
    pub internal_date: Option<DateTime<FixedOffset>>,
    /// Mailbox address the connector authenticates as, used to detect mail
    /// not addressed to the owner (issue #398).
    pub mailbox_address: Option<String>,
    /// IMAP `UIDVALIDITY` of the mailbox the message was fetched from.
    pub uid_validity: u32,
    /// IMAP UID of the message within that `UIDVALIDITY` epoch.
    pub uid: u32,
    /// `{uid_validity}:{uid}` provenance reference.
    pub raw_ref: String,
    /// Canonical user identity name (may be `None`).
    pub user_identity: Option<String>,
    /// Connector instance id for provenance.
    pub instance_id: i32,
    /// Connector type for provenance.
    pub connector_type: ConnectorType,
    /// Shared knowledge graph for `normalize_and_insert`.
    pub kg: Arc<KnowledgeGraph>,
    /// Shared LLM backend (system-queue routing).
    pub llm: Arc<dyn LlmBackend>,
    /// Shared durable ledger for terminal-failure recording.
    pub ledger: Arc<StdMutex<ProseRetryLedger>>,
    /// Per-connector retry budget (`llm_extraction_max_attempts`).
    pub max_attempts: u8,
}

/// Stateless handler for the `connector_item.remember` hook.
pub struct EmailExtractionHook;

impl EmailExtractionHook {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmailExtractionHook {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HookHandler for EmailExtractionHook {
    async fn run(&self, payload: Arc<dyn Any + Send + Sync>, ctx: HookContext) -> HookOutcome {
        let Some(payload) = payload.downcast_ref::<EmailExtractionPayload>() else {
            warn!("connector_item.remember hook: unexpected payload type; dropping instance");
            return HookOutcome::TerminalFailure;
        };
        let Some(message) = MessageParser::default().parse(&payload.raw) else {
            warn!(
                raw_ref = %payload.raw_ref,
                "connector_item.remember hook: could not parse RFC 822 message; dropping instance"
            );
            // This payload can never be retried into a parseable message, so
            // record the known-payload failure durably in the connector's
            // retry ledger — the health path reports terminal failures from
            // the ledger, and the item must not be re-staged on every cycle.
            payload.ledger.lock().unwrap().record_terminal(
                &payload.raw_ref,
                payload.uid_validity,
                payload.uid,
                ctx.attempt,
                "could not parse RFC 822 message".to_string(),
            );
            return HookOutcome::TerminalFailure;
        };
        let predicate_names = payload
            .kg
            .list_emit_eligible_relationship_type_names()
            .await
            .map_err(|error| {
                warn!(
                    raw_ref = %payload.raw_ref,
                    "loading closed taxonomy for email extraction failed: {error}"
                );
                error.to_string()
            });
        let Ok(predicate_names) = predicate_names else {
            return terminal_or_retry(payload, ctx, "closed taxonomy unavailable".to_string());
        };
        match extract_prose_facts(
            &payload.llm,
            payload.user_identity.as_deref(),
            &message,
            &payload.raw_ref,
            payload.internal_date,
            payload.mailbox_address.as_deref(),
            &predicate_names,
        )
        .await
        {
            Ok(outcome) => {
                let dropped = outcome.dropped as i64;
                let provenance = Provenance::connector(
                    payload.instance_id,
                    payload.connector_type,
                    ExtractionMethod::LlmExtraction,
                );
                for staged_fact in &outcome.staged {
                    if let Err(error) = payload
                        .kg
                        .stage_unrecognized_fact(
                            Some(payload.instance_id),
                            Some(&payload.raw_ref),
                            &staged_fact.relationship_type_raw,
                            &staged_fact.payload_json,
                            None,
                        )
                        .await
                    {
                        warn!(
                            raw_ref = %payload.raw_ref,
                            "staging unrecognized email fact failed: {error}"
                        );
                        return terminal_or_retry(
                            payload,
                            ctx,
                            format!("failed to stage unrecognized fact: {error}"),
                        );
                    }
                }

                match normalize_and_insert(&payload.kg, outcome.facts, provenance).await {
                    Ok(insert_outcome) => {
                        let accepted = (insert_outcome.inserted.len()
                            + insert_outcome.pending_confirmation.len())
                            as i64;
                        let dropped = dropped + insert_outcome.errors.len() as i64;
                        if dropped > 0 {
                            let total = accepted + dropped;
                            warn!(
                                raw_ref = %payload.raw_ref,
                                accepted,
                                dropped,
                                total,
                                "LLM email extraction dropped {dropped} of {total} facts",
                            );
                        }
                        debug!(
                            raw_ref = %payload.raw_ref,
                            inserted = insert_outcome.inserted.len(),
                            "connector_item.remember hook inserted facts"
                        );
                        // Cumulative acceptance counters (issue #508) so
                        // `mimir connector list` / `status` surfaces the
                        // drop rate instead of hiding it behind `items`.
                        if accepted + dropped > 0
                            && let Err(error) = payload
                                .kg
                                .record_connector_fact_counts(
                                    payload.instance_id,
                                    accepted,
                                    dropped,
                                    0,
                                )
                                .await
                        {
                            warn!(
                                raw_ref = %payload.raw_ref,
                                "recording connector fact acceptance counters failed: {error}"
                            );
                        }
                        HookOutcome::Success
                    }
                    Err(error) => {
                        warn!(
                            raw_ref = %payload.raw_ref,
                            "connector_item.remember hook insert failed: {error}"
                        );
                        terminal_or_retry(payload, ctx, error.to_string())
                    }
                }
            }
            Err(error) => {
                warn!(
                    raw_ref = %payload.raw_ref,
                    "connector_item.remember hook extraction failed: {error}"
                );
                terminal_or_retry(payload, ctx, error.to_string())
            }
        }
    }
}

/// Apply the per-connector retry budget: the final attempt records a
/// durable terminal failure; earlier attempts ask the runner to re-enqueue
/// with time-based backoff.
fn terminal_or_retry(
    payload: &EmailExtractionPayload,
    ctx: HookContext,
    error: String,
) -> HookOutcome {
    if ctx.attempt >= payload.max_attempts {
        payload.ledger.lock().unwrap().record_terminal(
            &payload.raw_ref,
            payload.uid_validity,
            payload.uid,
            ctx.attempt,
            error,
        );
        HookOutcome::TerminalFailure
    } else {
        HookOutcome::RetryableFailure
    }
}
