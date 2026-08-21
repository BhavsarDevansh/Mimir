//! Email connector LLM extraction — cascade layer 3 (C7 / #201).
//!
//! The deterministic layers ([`super::EmailConnector`] iMIP invites / #200, and
//! [`super::jsonld`] schema.org JSON-LD / #249) read machine-readable email.
//! This layer is the *last resort*: unstructured prose a deterministic layer
//! cannot read — a dentist's "see you Tuesday 3pm" with no `.ics`, a flight
//! confirmation in prose, a bank statement, a job offer — extracted under a
//! strict tool schema, validated in Rust, and funnelled through
//! [`normalize_and_insert`] with `extraction_method = LlmExtraction`.
//!
//! # Design rules
//!
//! - **Logic in Rust, not prompts.** The system prompt defines only the
//!   extractor's role and high-level goal. Every structural decision the LLM
//!   could get wrong — entity types, temporal bounds, the event-type hint,
//!   recurrence, the location overlay — is validated in Rust against the typed
//!   enums before a [`NormalizedFact`] is built. An invalid field is warned
//!   and dropped, never trusted.
//! - **Spam is classified in Rust, not by the LLM.** [`is_likely_spam`] skips
//!   the LLM call entirely for obvious bulk-marketing infrastructure mail
//!   (messages sent through known email-service-provider domains). Everything
//!   else reaches the LLM, which returns an empty `facts` array when the prose
//!   carries no real-world facts — so "some emails → no facts" judgment lives
//!   in the LLM, while *obvious* spam never costs a call.
//! - **System-queue routing.** Every LLM call goes through
//!   [`LlmBackend::system_chat_message`], placing it on the shared
//!   `LlmWorkerPool`'s system queue (priority below user chat) so a
//!   one-call-at-a-time provider is never starved by an extraction burst and a
//!   queued user chat preempts a waiting connector call (#201 acceptance).
//! - **One call per email** (small per-item calls) so the user queue preempts
//!   between calls.
//! - **User-scoped facts** authored against the injected `user_identity` so
//!   they resolve to the canonical user entity (matching the C4 / #198 and
//!   JSON-LD / #249 layers); [`canonicalise_subject`] normalises generic
//!   pronouns/"the user" to the exact identity name.
//!
//! # Module layout
//!
//! - [`schema`] — LLM tool schema, wire types, and system prompt.
//! - [`parse`] — LLM-output parsing with Rust-side validation.
//! - [`message`] — spam classification, body text, subject canonicalisation.
//! - [`retry`] — the durable, bounded retry ledger for failed prose
//!   extraction (issue #262): attempt counts, cycle backoff, terminal
//!   failures, and the persisted ledger format.

mod hook;
mod message;
mod parse;
pub(crate) mod retry;
mod schema;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::sync::Arc;

use mail_parser::Message;
use mimir_core::llm::{LlmBackend, Message as LlmMessage};
use mimir_knowledge::normalize::NormalizedFact;
use tracing::{debug, warn};

use crate::connector::ConnectorError;
pub use crate::email::llm::hook::EmailExtractionHook;
pub(crate) use crate::email::llm::hook::EmailExtractionPayload;
use crate::email::llm::message::{body_text, from_address, is_likely_spam};
use crate::email::llm::parse::{build_fact, parse_output};
pub(crate) use crate::email::llm::retry::{
    DEFAULT_MAX_LLM_EXTRACTION_ATTEMPTS, ProseRetryLedger, health_with_terminal,
};
use crate::email::llm::schema::{build_system_prompt, email_extraction_tool_schema};

pub(crate) async fn extract_prose_facts(
    backend: &Arc<dyn LlmBackend>,
    user_identity: Option<&str>,
    message: &Message<'_>,
    raw_ref: &str,
) -> Result<Vec<NormalizedFact>, ConnectorError> {
    let from = from_address(message);
    let has_unsubscribe = message.header("List-Unsubscribe").is_some();
    if is_likely_spam(from.as_deref(), has_unsubscribe) {
        debug!(raw_ref, from = ?from, "skipping LLM layer: bulk-marketing sender");
        return Ok(Vec::new());
    }

    let subject = message.subject().unwrap_or("").to_string();
    let Some(body) = body_text(message) else {
        debug!(
            raw_ref,
            "no decodable text body for LLM extraction; skipping"
        );
        return Ok(Vec::new());
    };

    let prompt = build_system_prompt(user_identity);
    let user_turn = format!(
        "From: {}\nSubject: {}\n\nBody:\n{}",
        from.unwrap_or_default(),
        subject,
        truncate_body(&body),
    );
    let messages = vec![LlmMessage::system(prompt), LlmMessage::user(user_turn)];

    // Propagate LLM/parse failures so `extract()` can re-stage the raw email
    // for retry instead of recording a silent empty success.
    let assistant = backend
        .system_chat_message(messages, Some(vec![email_extraction_tool_schema().clone()]))
        .await
        .map(|(msg, _usage)| msg)?;
    let output = parse_output(assistant)?;

    let mut facts = Vec::with_capacity(output.facts.len());
    for fact in output.facts {
        match build_fact(fact, user_identity, raw_ref) {
            Ok(f) => facts.push(f),
            Err(error) => warn!(raw_ref, "dropping invalid LLM email fact: {error}"),
        }
    }
    debug!(
        raw_ref,
        n = facts.len(),
        "LLM email extraction produced facts"
    );
    Ok(facts)
}

/// Cap the body sent to the LLM to bound token cost. 8 KiB of prose is far more
/// than any single confirmation needs; longer marketing/transactional bodies
/// are truncated with a marker so the LLM still sees the salient headers and
/// opening text. The bound is on UTF-8 bytes (truncated on a char boundary).
const MAX_BODY_BYTES: usize = 8 * 1024;

fn truncate_body(body: &str) -> String {
    if body.len() <= MAX_BODY_BYTES {
        return body.to_string();
    }
    // Bound by char boundary to avoid splitting a UTF-8 codepoint.
    let mut end = MAX_BODY_BYTES;
    while end < body.len() && !body.is_char_boundary(end) {
        end += 1;
    }
    let head = &body[..end];
    format!("{head}\n[…] [body truncated]")
}
