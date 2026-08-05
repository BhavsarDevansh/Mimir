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

use std::sync::Arc;

use mail_parser::{Address, Message};
use mimir_core::llm::{LlmBackend, Message as LlmMessage};
use mimir_knowledge::extract::{
    ExtractedLocation, Temporal, parse_entity_type, parse_event_type, parse_location,
    parse_recurrence, parse_temporal_bound,
};
use mimir_knowledge::models::enums::RecurrenceType;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::NormalizedFact;
use tracing::{debug, warn};

use crate::connector::ConnectorError;

/// Name of the LLM tool the extractor must call. Kept as a single constant so
/// the schema and the response-validation step agree on the expected name; a
/// tool call whose `function.name` differs is rejected (see [`parse_output`]).
const EMAIL_EXTRACTION_TOOL_NAME: &str = "extract_email_facts";

// ---------------------------------------------------------------------------
// Wire types (LLM tool output)
// ---------------------------------------------------------------------------

/// One fact emitted by the LLM for a single email. Mirrors the conversational
/// [`mimir_knowledge::extract::ExtractedFact`] minus the conversational-only
/// fields (`classification`, `correction_scope`), plus an optional
/// `event_type` hint that Rust maps onto [`NormalizedFact::event_type`].
#[derive(Debug, Clone, serde::Deserialize)]
struct EmailFact {
    subject: String,
    subject_type: String,
    relationship_type: String,
    object: String,
    object_is_entity: bool,
    #[serde(default)]
    object_type: Option<String>,
    #[serde(default)]
    temporal: Option<Temporal>,
    /// Event kind hint (Birthday / Appointment / Deadline / Task / Reminder /
    /// Custom). Validated against the [`EventType`] enum in Rust; an
    /// unrecognised value is dropped (the overlay derives the type).
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    recurrence: Option<String>,
    #[serde(default)]
    requires_user_action: Option<bool>,
    #[serde(default)]
    is_sensitive: bool,
    #[serde(default)]
    location: Option<ExtractedLocation>,
}

/// Wrapper the tool returns: an empty `facts` array means the email carried no
/// real-world facts (marketing, newsletter, or nothing actionable).
#[derive(Debug, Clone, serde::Deserialize)]
struct EmailFactOutput {
    facts: Vec<EmailFact>,
}

// ---------------------------------------------------------------------------
// Tool schema
// ---------------------------------------------------------------------------

/// The strict JSON-Schema tool the LLM must call to return extracted facts.
///
/// Deliberately narrow: connector prose extraction does not classify facts as
/// Explicit/Casual/Correction (those are conversational-only) and never emits
/// corrections. The `event_type` enum is closed and matched against the Rust
/// [`EventType`] enum, so the LLM cannot invent a kind.
pub(crate) fn email_extraction_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": EMAIL_EXTRACTION_TOOL_NAME,
            "description": "Extract real-world facts about the user that the email's prose conveys. Do NOT model the email itself as a fact; extract the underlying event, booking, date, address, transaction, or commitment. Return an empty facts array for marketing, newsletters, or emails with no usable facts.",
            "parameters": {
                "type": "object",
                "properties": {
                    "facts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "subject": {
                                    "type": "string",
                                    "description": "The entity the fact is about. For facts about the mailbox owner, use their name exactly as given in the task."
                                },
                                "subject_type": {
                                    "type": "string",
                                    "enum": ["Person","Place","Event","Object","Concept","Organization","Activity","DateTime"],
                                    "description": "Entity type of the subject."
                                },
                                "relationship_type": {
                                    "type": "string",
                                    "description": "The relationship or property being asserted (e.g. has_flight, has_event, has_appointment, owes, lives_at)."
                                },
                                "object": {
                                    "type": "string",
                                    "description": "The value or target of the relationship_type."
                                },
                                "object_is_entity": {
                                    "type": "boolean",
                                    "description": "Whether the object is an entity (true) or a literal string (false)."
                                },
                                "object_type": {
                                    "type": "string",
                                    "enum": ["Person","Place","Event","Object","Concept","Organization","Activity","DateTime"],
                                    "description": "Entity type of the object, if object_is_entity is true."
                                },
                                "temporal": {
                                    "type": "object",
                                    "properties": {
                                        "valid_from": {"type": "string", "description": "ISO-8601 datetime when this fact becomes true."},
                                        "valid_until": {"type": "string", "description": "ISO-8601 datetime when this fact ceases to be true."}
                                    }
                                },
                                "event_type": {
                                    "type": "string",
                                    "enum": ["Birthday","Appointment","Deadline","Task","Reminder","Custom"],
                                    "description": "Optional event kind hint for timed/recurring/action items. Omit for non-event facts."
                                },
                                "recurrence": {
                                    "type": "string",
                                    "enum": ["none","daily","weekly","monthly","yearly"],
                                    "description": "How the date recurs, for recurring facts (birthdays, anniversaries). Omit or 'none' for one-time facts."
                                },
                                "requires_user_action": {
                                    "type": "boolean",
                                    "description": "True for tasks/deadlines the user must complete. False or omit for reminders that auto-complete."
                                },
                                "is_sensitive": {
                                    "type": "boolean",
                                    "description": "Whether this fact involves health, financial, relationship, or other sensitive topics."
                                },
                                "location": {
                                    "type": "object",
                                    "description": "Optional. Present only for 'where' facts.",
                                    "properties": {
                                        "location_type": {"type": "string", "enum": ["Home","Work","Visited","Origin","Current"]},
                                        "address": {"type": "string"},
                                        "latitude": {"type": "number"},
                                        "longitude": {"type": "number"},
                                        "timezone": {"type": "string"}
                                    },
                                    "required": ["location_type"]
                                }
                            },
                            "required": ["subject","subject_type","relationship_type","object","object_is_entity"]
                        }
                    }
                },
                "required": ["facts"]
            }
        }
    })
}

// ---------------------------------------------------------------------------
// System prompt (role + high-level goal only — no conditional logic)
// ---------------------------------------------------------------------------

/// Build the extractor system prompt. The user's name is injected so facts
/// about the mailbox owner author against the canonical identity; everything
/// else (validation, control flow) is Rust's job.
fn build_system_prompt(user_identity: Option<&str>) -> String {
    let owner = user_identity.unwrap_or("the mailbox owner");
    format!(
        "You are Mimir's email fact extractor. Read the provided email and \
extract the real-world facts it conveys about {owner} — appointments, flights, \
bookings, deadlines, tasks, addresses, dates, financial transactions, job \
offers, and other concrete facts about {owner}. Extract the underlying \
event or thing, not the email itself (do not emit 'received email from' \
facts). If the email is marketing, a newsletter, or carries no real-world \
facts about {owner}, return an empty facts array. For facts about {owner}, \
use the exact name '{owner}' as the subject. Emit the facts via the \
extract_email_facts tool."
    )
}

// ---------------------------------------------------------------------------
// Spam pre-filter (deterministic, Rust-side)
// ---------------------------------------------------------------------------

/// Domains of bulk email-service providers. Mail delivered through these is
/// Domains of bulk *marketing* platforms — providers whose product is
/// newsletter/campaign delivery (Mailchimp, HubSpot, …). Mail sent from these
/// is marketing, so it is skipped before any LLM call.
/// General-purpose email-service providers that also deliver transactional
/// receipts, bookings, and account notices (SendGrid, Mailgun, Postmark,
/// Amazon SES, Mandrill, SparkPost, Brevo) are deliberately *not* listed
/// here: a booking or bank statement routed through them must still reach
/// the LLM. Those messages are skipped only when they carry an explicit bulk
/// signal (the `List-Unsubscribe` header — see [`is_likely_spam`]).
const MARKETING_SENDER_DOMAINS: &[&str] = &[
    "mailchimp.com",
    "hubspot.com",
    "mailerlite.com",
    "constantcontact.com",
    "campaignmonitor.com",
    "elasticemail.com",
    "mail.marketing",
    "email-od.com",
];

/// Return the sender domain (lower-cased) from a `From` header address.
fn sender_domain(from: Option<&str>) -> Option<String> {
    let addr = from?;
    let domain = addr.rsplit_once('@').map(|(_, d)| d)?;
    let domain = domain.trim().trim_end_matches('>').to_ascii_lowercase();
    if domain.is_empty() {
        None
    } else {
        Some(domain)
    }
}

/// Conservative deterministic spam gate: skip the LLM only for obvious
/// Conservative deterministic spam gate: skip the LLM only for obvious
/// bulk-marketing mail. A message is skipped when either (a) it carries a
/// `List-Unsubscribe` header — the universal bulk-mail signal (RFC 8058) that
/// transactional receipts, bookings, and account notices never carry — or
/// (b) its sender domain is a pure marketing platform (see
/// [`MARKETING_SENDER_DOMAINS`]). Provider origin alone never skips a
/// message, so a transactional email routed through a general-purpose ESP
/// (SendGrid, Mailgun, Postmark, Amazon SES) still reaches the LLM.
/// Everything else reaches the LLM, which decides "no facts" by returning an
/// empty array. Returns `true` when the message should be skipped.
pub(crate) fn is_likely_spam(from_addr: Option<&str>, has_unsubscribe: bool) -> bool {
    // Explicit bulk signal: a `List-Unsubscribe` header is present only on
    // bulk mail (newsletters, campaigns, promotional broadcasts). This gate
    // never drops transactional mail, which does not carry one.
    if has_unsubscribe {
        return true;
    }
    let Some(domain) = sender_domain(from_addr) else {
        return false;
    };
    // Exact ESP domain, or a subdomain of one (`mc.us1.sendgrid.net`). The
    // `strip_suffix` + `ends_with('.')` check avoids a per-domain allocation
    // that `format!(".{esp}")` would incur on every email.
    MARKETING_SENDER_DOMAINS.iter().any(|esp| {
        domain == *esp
            || domain
                .strip_suffix(esp)
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

// ---------------------------------------------------------------------------
// Email content extraction (From / Subject / body)
// ---------------------------------------------------------------------------

/// Extract the first `From` address string from a parsed message.
fn from_address(message: &Message<'_>) -> Option<String> {
    match message.from()? {
        Address::List(addrs) => addrs
            .iter()
            .find_map(|a| a.address.as_ref().map(|c| c.to_string())),
        Address::Group(groups) => groups
            .iter()
            .flat_map(|g| g.addresses.iter())
            .find_map(|a| a.address.as_ref().map(|c| c.to_string())),
    }
}

/// Best-effort plain-text body: the first text/plain body, or the first HTML
/// body stripped of markup when no text/plain part exists.
fn body_text(message: &Message<'_>) -> Option<String> {
    if let Some(text) = message.body_text(0) {
        let text = text.into_owned();
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    message
        .body_html(0)
        .map(|html| strip_html(&html))
        .filter(|t| !t.trim().is_empty())
}

/// Naive HTML-to-text: drop tags, decode a few common entities, collapse
/// whitespace. Good enough to hand the LLM prose from an HTML-only email; the
/// LLM still parses prose, not structure.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                if in_tag {
                    out.push(' ');
                    in_tag = false;
                }
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    collapse_whitespace(&decoded)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Subject canonicalisation (logic in Rust, not prompts)
// ---------------------------------------------------------------------------

/// Generic self-references the LLM may use for the mailbox owner. These are
/// normalised to the exact `user_identity` so the fact resolves to the
/// canonical user entity instead of a "the user" / "I" entity.
const GENERIC_USER_REFERENCES: &[&str] = &["i", "me", "myself", "user", "the user"];

/// When `user_identity` is set and the LLM's subject is a generic
/// self-reference (or a case-insensitive match of the identity name), return
/// the exact identity name so entity resolution hits the canonical user.
/// Other subjects pass through unchanged.
fn canonicalise_subject(subject: &str, user_identity: Option<&str>) -> String {
    let Some(identity) = user_identity else {
        return subject.to_string();
    };
    let trimmed = subject.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == identity.to_ascii_lowercase() || GENERIC_USER_REFERENCES.contains(&lower.as_str()) {
        identity.to_string()
    } else {
        subject.to_string()
    }
}

// ---------------------------------------------------------------------------
// Output parsing + validation → NormalizedFact
// ---------------------------------------------------------------------------

/// Parse the assistant message into [`EmailFactOutput`]. Supports tool-call
/// output and a fallback JSON parse for backends that emit raw JSON.
fn parse_output(message: LlmMessage) -> Result<EmailFactOutput, ConnectorError> {
    if let Some(tool_calls) = message.tool_calls {
        // A single email needs exactly one `extract_email_facts` call. Reject
        // a multi-call completion (the prompt asks for one call only) and an
        // unexpected tool name, so arguments from a different function never
        // become email facts.
        if tool_calls.len() > 1 {
            return Err(ConnectorError::Parse(format!(
                "LLM returned {n} tool calls; expected exactly one \
                 `{EMAIL_EXTRACTION_TOOL_NAME}` call.",
                n = tool_calls.len()
            )));
        }
        let first = tool_calls
            .into_iter()
            .next()
            .ok_or_else(|| ConnectorError::Parse("LLM tool call list was empty.".into()))?;
        if first.function.name != EMAIL_EXTRACTION_TOOL_NAME {
            return Err(ConnectorError::Parse(format!(
                "LLM returned tool call `{name}`; expected \
                 `{EMAIL_EXTRACTION_TOOL_NAME}`.",
                name = first.function.name
            )));
        }
        return serde_json::from_str(&first.function.arguments).map_err(|e| {
            ConnectorError::Parse(format!(
                "failed to parse {EMAIL_EXTRACTION_TOOL_NAME} arguments: {e}"
            ))
        });
    }
    let text = message.content.trim();
    if text.is_empty() {
        return Err(ConnectorError::Parse(
            "LLM emitted no tool call for email extraction.".into(),
        ));
    }
    let json_text = strip_code_fence(text);
    serde_json::from_str::<EmailFactOutput>(&json_text).map_err(|e| {
        ConnectorError::Parse(format!(
            "LLM response not parseable as {{\"facts\": [...]}}: {e}; head: {}",
            json_text.chars().take(200).collect::<String>()
        ))
    })
}

/// Return the JSON text from an assistant reply, stripping a
/// ```fence``` if the model wrapped its output. Owned (no `Box::leak`):
/// this runs on every fallback parse, so a leaked allocation per LLM reply
/// would grow memory over a long sync.
fn strip_code_fence(text: &str) -> String {
    let text = text.trim();
    if !text.starts_with("```") {
        return text.to_string();
    }
    text.lines()
        .skip_while(|l| l.starts_with("```"))
        .take_while(|l| !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a single [`NormalizedFact`] from one LLM-emitted [`EmailFact`].
/// Per-field validation matches the conversational path: an invalid entity
/// type / event type / location is warned and dropped (or the whole fact
/// skipped when the subject type is invalid), never trusted.
fn build_fact(
    fact: EmailFact,
    user_identity: Option<&str>,
    raw_ref: &str,
) -> Result<NormalizedFact, ConnectorError> {
    let subject_type = parse_entity_type(&fact.subject_type).map_err(|e| {
        ConnectorError::Parse(format!("invalid subject_type {:?}: {e}", fact.subject_type))
    })?;
    let object_type = fact
        .object_type
        .as_deref()
        .map(parse_entity_type)
        .transpose()
        .map_err(|e| ConnectorError::Parse(format!("invalid object_type: {e}")))?;

    let valid_from =
        parse_temporal_bound(fact.temporal.as_ref().and_then(|t| t.valid_from.as_deref()));
    let valid_until = parse_temporal_bound(
        fact.temporal
            .as_ref()
            .and_then(|t| t.valid_until.as_deref()),
    );
    let recurrence = fact
        .recurrence
        .as_deref()
        .and_then(parse_recurrence)
        .unwrap_or(RecurrenceType::None);
    let requires_user_action = fact.requires_user_action.unwrap_or(false);

    // Event-type hint validated against the enum; unrecognised → None (the
    // overlay derives the type). Never trusted raw.
    let event_type = fact.event_type.as_deref().and_then(parse_event_type);
    if fact.event_type.is_some() && event_type.is_none() {
        warn!(
            raw_ref,
            "LLM emitted unrecognised event_type {:?}; dropping hint", fact.event_type
        );
    }

    let location = fact
        .location
        .as_ref()
        .and_then(|loc| match parse_location(loc) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                warn!(
                    raw_ref,
                    "invalid location overlay for email fact; ignoring: {error}"
                );
                None
            }
        });

    Ok(NormalizedFact {
        source_type: SourceType::Connector,
        subject: canonicalise_subject(&fact.subject, user_identity),
        subject_type,
        relationship_type: fact.relationship_type,
        object: fact.object,
        object_is_entity: fact.object_is_entity,
        object_type,
        valid_from,
        valid_until,
        is_sensitive: fact.is_sensitive,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence,
        requires_user_action,
        raw_reference: Some(raw_ref.to_string()),
        extraction_method: Some(ExtractionMethod::LlmExtraction),
        event_type,
        location,
    })
}

// ---------------------------------------------------------------------------
// Public entrypoint
// ---------------------------------------------------------------------------

/// Extract prose facts from a single email via the shared LLM backend's
/// system queue. Returns the validated [`NormalizedFact`]s — an *empty* vec
/// is a legitimate result (spam, no decodable body, or the LLM correctly
/// found no facts). Per-fact validation errors are tolerated (warned +
/// skipped) so one bad fact never aborts the email.
///
/// Retryable LLM-level failures (queue full, network, provider, or parse
/// error) are returned as [`ConnectorError`] rather than silently converted
/// into an empty vec. The connector's `extract()` step re-stages the
/// affected raw email so the next extraction cycle retries it; an empty vec
/// returned as success would lose the message forever (the buffer was
/// drained and the IMAP cursor advanced). A bounded retry / terminal-failure
/// policy is follow-up work.
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
    let tool = email_extraction_tool_schema();

    // Propagate LLM/parse failures so `extract()` can re-stage the raw email
    // for retry instead of recording a silent empty success.
    let assistant = backend
        .system_chat_message(messages, Some(vec![tool]))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mimir_core::llm::MockLlmClient;

    fn parse(bytes: &[u8]) -> mail_parser::Message<'_> {
        mail_parser::MessageParser::default()
            .parse(bytes)
            .expect("parse")
    }

    fn email(from: &str, subject: &str, body: &str) -> Vec<u8> {
        format!(
            "From: {from}\r\nSubject: {subject}\r\n\
             Content-Type: text/plain; charset=\"utf-8\"\r\n\r\n{body}"
        )
        .into_bytes()
    }

    fn mock_with_tool_response(json: &str) -> MockLlmClient {
        let tool_call = mimir_core::llm::ToolCall {
            index: 0,
            id: "call_1".into(),
            call_type: "function".into(),
            function: mimir_core::llm::FunctionCall {
                name: "extract_email_facts".into(),
                arguments: json.into(),
            },
        };
        let message = LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        MockLlmClient::builder()
            .push_chat_message(message, Default::default())
            .build()
    }

    #[test]
    fn spam_filter_skips_marketing_senders_and_unsubscribe_signal() {
        // Pure marketing platforms are skipped by sender domain alone.
        assert!(is_likely_spam(Some("promo@mailchimp.com"), false));
        assert!(is_likely_spam(Some("news@hubspot.com"), false));
        // General-purpose ESPs (SendGrid, Mailgun, Postmark, Amazon SES) are
        // NOT skipped by domain alone — a transactional receipt routed
        // through them must reach the LLM.
        assert!(!is_likely_spam(Some("news@mc.us1.sendgrid.net"), false));
        assert!(!is_likely_spam(Some("receipt@mailgun.org"), false));
        assert!(!is_likely_spam(Some("no-reply@amazonses.com"), false));
        // The same ESP IS skipped when it carries a bulk signal.
        assert!(is_likely_spam(Some("news@mc.us1.sendgrid.net"), true));
        // Non-ESP senders are never spam by domain; an unsubscribe header
        // still marks them bulk.
        assert!(!is_likely_spam(Some("statements@barclays.co.uk"), false));
        assert!(!is_likely_spam(Some("reservations@ba.com"), false));
        assert!(is_likely_spam(Some("news@example.com"), true));
        assert!(!is_likely_spam(None, false));
    }

    fn tool_call(name: &str, args: &str) -> LlmMessage {
        let tool_call = mimir_core::llm::ToolCall {
            index: 0,
            id: "call_1".into(),
            call_type: "function".into(),
            function: mimir_core::llm::FunctionCall {
                name: name.into(),
                arguments: args.into(),
            },
        };
        LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        }
    }

    #[test]
    fn parse_output_rejects_unexpected_tool_name() {
        let msg = tool_call("summarise_email", r#"{"facts": []}"#);
        assert!(parse_output(msg).is_err());
    }

    #[test]
    fn parse_output_rejects_multiple_tool_calls() {
        let first = mimir_core::llm::ToolCall {
            index: 0,
            id: "call_1".into(),
            call_type: "function".into(),
            function: mimir_core::llm::FunctionCall {
                name: "extract_email_facts".into(),
                arguments: r#"{"facts": []}"#.into(),
            },
        };
        let second = mimir_core::llm::ToolCall {
            index: 1,
            id: "call_2".into(),
            call_type: "function".into(),
            function: mimir_core::llm::FunctionCall {
                name: "other_tool".into(),
                arguments: "{}".into(),
            },
        };
        let msg = LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(vec![first, second]),
            tool_call_id: None,
        };
        assert!(parse_output(msg).is_err());
    }

    #[test]
    fn parse_output_accepts_expected_tool_name() {
        let msg = tool_call("extract_email_facts", r#"{"facts": []}"#);
        assert!(parse_output(msg).is_ok());
    }

    #[test]
    fn canonicalise_subject_maps_generic_pronouns_to_identity() {
        let id = Some("Devansh");
        assert_eq!(canonicalise_subject("I", id), "Devansh");
        assert_eq!(canonicalise_subject("the user", id), "Devansh");
        assert_eq!(canonicalise_subject("devansh", id), "Devansh");
        assert_eq!(canonicalise_subject("BA1234", id), "BA1234");
        assert_eq!(canonicalise_subject("me", None), "me");
    }

    #[test]
    fn strip_html_removes_tags_and_decodes_entities() {
        let out = strip_html("<p>See you <b>Tuesday 3pm</b> &amp; bring &nbsp;records</p>");
        assert_eq!(out, "See you Tuesday 3pm & bring records");
    }

    #[tokio::test]
    async fn spam_email_skips_llm_call_entirely() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email(
            "promo@mailchimp.com",
            "50% off everything",
            "Sale ends Sunday",
        );
        let msg = parse(&bytes);
        let facts = extract_prose_facts(&backend, Some("Devansh"), &msg, "17:1")
            .await
            .expect("spam -> empty facts");
        assert!(facts.is_empty());
        // No LLM call was made (the mock would error with no queued response
        // if the call had been issued, and system_chat_calls stays empty).
        assert!(mock.system_chat_calls().is_empty());
    }

    #[tokio::test]
    async fn no_fact_email_yields_empty_facts_array() {
        let mock = Arc::new(mock_with_tool_response(r#"{"facts": []}"#));
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email(
            "news@example.com",
            "Weekly digest",
            "Here are this week's links.",
        );
        let msg = parse(&bytes);
        let facts = extract_prose_facts(&backend, Some("Devansh"), &msg, "17:2")
            .await
            .expect("no-fact -> empty facts");
        assert!(facts.is_empty());
        // The call routed through the system queue, not the user queue.
        assert_eq!(mock.system_chat_calls().len(), 1);
        assert!(mock.chat_calls().is_empty());
    }

    #[tokio::test]
    async fn dentist_appointment_produces_typed_fact() {
        let mock = Arc::new(mock_with_tool_response(
            r#"{"facts": [{
                "subject": "the user",
                "subject_type": "Person",
                "relationship_type": "has_appointment",
                "object": "Dentist check-up",
                "object_is_entity": true,
                "object_type": "Event",
                "temporal": {"valid_from": "2026-08-11T14:00:00Z"},
                "event_type": "Appointment"
            }]}"#,
        ));
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email(
            "reception@dentalclinic.com",
            "Your appointment",
            "See you Tuesday 3pm. Please arrive 10 minutes early.",
        );
        let msg = parse(&bytes);
        let facts = extract_prose_facts(&backend, Some("Devansh"), &msg, "17:42")
            .await
            .expect("typed fact");
        assert_eq!(facts.len(), 1, "{facts:?}");
        let f = &facts[0];
        assert_eq!(f.subject, "Devansh", "subject canonicalised to identity");
        assert_eq!(f.relationship_type, "has_appointment");
        assert_eq!(f.extraction_method, Some(ExtractionMethod::LlmExtraction));
        assert_eq!(
            f.event_type,
            Some(mimir_knowledge::models::enums::EventType::Appointment)
        );
        assert_eq!(f.raw_reference.as_deref(), Some("17:42"));
        assert_eq!(f.source_type, SourceType::Connector);
    }

    #[tokio::test]
    async fn invalid_event_type_hint_is_dropped_not_trusted() {
        let mock = Arc::new(mock_with_tool_response(
            r#"{"facts": [{
                "subject": "me",
                "subject_type": "Person",
                "relationship_type": "has_event",
                "object": "Mystery",
                "object_is_entity": true,
                "object_type": "Event",
                "event_type": "Surprise"
            }]}"#,
        ));
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email("a@example.com", "Hi", "body");
        let msg = parse(&bytes);
        let facts = extract_prose_facts(&backend, Some("Devansh"), &msg, "17:3")
            .await
            .expect("dropped event_type");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].event_type, None, "unrecognised event_type dropped");
        assert_eq!(mock.system_chat_calls().len(), 1);
    }

    #[tokio::test]
    async fn invalid_subject_type_drops_the_fact() {
        let mock = Arc::new(mock_with_tool_response(
            r#"{"facts": [{
                "subject": "x",
                "subject_type": "Alien",
                "relationship_type": "has_event",
                "object": "y",
                "object_is_entity": true
            }]}"#,
        ));
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email("a@example.com", "Hi", "body");
        let msg = parse(&bytes);
        let facts = extract_prose_facts(&backend, None, &msg, "17:4")
            .await
            .expect("dropped subject_type");
        assert!(facts.is_empty(), "invalid subject_type drops the fact");
        assert_eq!(mock.system_chat_calls().len(), 1);
    }

    #[tokio::test]
    async fn unparseable_llm_output_is_a_retryable_error() {
        let mock = Arc::new(mock_with_tool_response(r#"not json at all"#));
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email("a@example.com", "Hi", "body");
        let msg = parse(&bytes);
        let result = extract_prose_facts(&backend, Some("Devansh"), &msg, "17:5").await;
        assert!(
            result.is_err(),
            "unparseable LLM output must not be a silent empty success"
        );
    }

    #[tokio::test]
    async fn llm_backend_error_is_a_retryable_error() {
        // A queue-full / network / provider failure is a retryable error, not
        // an empty fact list — so the connector can re-stage the raw email.
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_error(mimir_core::llm::LlmError::QueueFull)
                .build(),
        );
        let backend: Arc<dyn LlmBackend> = mock.clone();
        let bytes = email("a@example.com", "Hi", "body");
        let msg = parse(&bytes);
        let result = extract_prose_facts(&backend, Some("Devansh"), &msg, "17:6").await;
        assert!(result.is_err());
    }
}
