//! Email envelope: the message-context surface shared by every extraction
//! layer (issue #398).
//!
//! An email's facts are only as trustworthy as its context: a two-year-old
//! reminder must not surface as a current action item, and bulk, forwarded,
//! or misdirected mail must not author obligations for the mailbox owner.
//! This module derives that context once per message — dates, sender,
//! recipients, and the deterministic spam / forwarding / wrong-recipient
//! signals — and binds every extracted fact to it. All classification is
//! deterministic Rust: no LLM is consulted for context.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use mail_parser::{Address, Message};
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::normalize::NormalizedFact;

use crate::email::llm::message::{body_text, is_likely_spam};

/// How long an actionable item extracted from an email stays actionable
/// when the message states no deadline of its own. The window anchors at
/// the email's own date, so urgency decays with the email's age: a
/// two-year-old reminder's window is already in the past and the fact can
/// never surface as a current action item (issue #398).
pub(crate) const ACTIONABLE_WINDOW_DAYS: i64 = 30;

/// The message envelope every extraction layer sees (issue #398): dates,
/// sender, recipients, and the deterministic spam / forwarding /
/// wrong-recipient signals derived from headers and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmailEnvelope {
    /// RFC 5322 `Date` header, converted to UTC.
    pub sent_date: Option<DateTime<Utc>>,
    /// IMAP `INTERNALDATE` (server receive time), converted to UTC.
    pub received_date: Option<DateTime<Utc>>,
    /// `From` address.
    pub from: Option<String>,
    /// `To` addresses.
    pub to: Vec<String>,
    /// `Cc` addresses.
    pub cc: Vec<String>,
    /// `Reply-To` addresses.
    pub reply_to: Vec<String>,
    /// Subject line.
    pub subject: String,
    /// Best-effort plain-text body, extracted once and shared by the
    /// forwarded-body check and the LLM prompt. `None` when the message has
    /// no decodable text body, or when the subject already marks the message
    /// as forwarded (the body pass is then skipped).
    pub body: Option<String>,
    /// `List-Unsubscribe` header present (RFC 8058 bulk-mail signal).
    pub has_list_unsubscribe: bool,
    /// Deterministic bulk-marketing gate result ([`is_likely_spam`]).
    pub is_spam: bool,
    /// The subject prefix or body separator marks the message as forwarded.
    pub is_forwarded: bool,
    /// The mailbox owner's address appears in neither `To` nor `Cc`.
    pub is_wrong_recipient: bool,
}

impl EmailEnvelope {
    /// Derive the envelope from a parsed message plus the IMAP
    /// `INTERNALDATE` (which is not part of the RFC 5322 headers) and the
    /// mailbox address the connector authenticates as.
    pub(crate) fn from_message(
        message: &Message<'_>,
        internal_date: Option<DateTime<FixedOffset>>,
        mailbox_address: Option<&str>,
    ) -> Self {
        let sent_date = message.date().and_then(datetime_to_utc);
        let received_date = internal_date.map(|d| d.with_timezone(&Utc));
        let to = collect_addresses(message.to());
        let cc = collect_addresses(message.cc());
        let from = collect_addresses(message.from()).into_iter().next();
        let has_list_unsubscribe = message.header("List-Unsubscribe").is_some();
        let subject = message.subject().unwrap_or("").to_string();
        // The body is only needed to detect inline forwards; a subject that
        // already says "Fwd:" saves the body-text pass.
        let subject_forwarded = forwarded_subject_prefix(&subject);
        let body = (!subject_forwarded).then(|| body_text(message)).flatten();
        let is_forwarded = subject_forwarded || is_forwarded_body(body.as_deref());
        let is_wrong_recipient = wrong_recipient(&to, &cc, mailbox_address);
        let is_spam = is_likely_spam(from.as_deref(), has_list_unsubscribe);

        Self {
            sent_date,
            received_date,
            from,
            to,
            cc,
            reply_to: collect_addresses(message.reply_to()),
            subject,
            body,
            has_list_unsubscribe,
            is_spam,
            is_forwarded,
            is_wrong_recipient,
        }
    }
}

/// Convert a parsed RFC 5322 date to UTC (chrono), or `None` when the
/// parsed date is invalid or out of range.
fn datetime_to_utc(date: &mail_parser::DateTime) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(date.to_timestamp(), 0)
}

/// The addr-specs of a recipient header (`To`, `Cc`, `Reply-To`), with
/// display names dropped.
fn collect_addresses(address: Option<&Address<'_>>) -> Vec<String> {
    let Some(address) = address else {
        return Vec::new();
    };
    match address {
        Address::List(addrs) => addrs
            .iter()
            .filter_map(|a| a.address.as_ref().map(|c| c.to_string()))
            .collect(),
        Address::Group(groups) => groups
            .iter()
            .flat_map(|g| g.addresses.iter())
            .filter_map(|a| a.address.as_ref().map(|c| c.to_string()))
            .collect(),
    }
}

/// Whether the subject line marks the message as forwarded: the common
/// `Fwd:` / `FW:` prefix used by Gmail, Outlook, and Apple Mail.
fn forwarded_subject_prefix(subject: &str) -> bool {
    let lower = subject.trim().to_ascii_lowercase();
    lower.starts_with("fwd:") || lower.starts_with("fw:")
}

/// Whether the body carries the standard "Forwarded message" separator
/// used by Gmail and most clients for inline forwards.
fn is_forwarded_body(body: Option<&str>) -> bool {
    body.is_some_and(|b| b.contains("---------- Forwarded message ----------"))
}

/// Whether the mailbox owner's address appears in neither `To` nor `Cc`
/// (the mail was BCC'd or misdirected, so it is not addressed to them).
/// Only email-looking mailbox addresses participate — an IMAP username
/// without an `@` can never match a recipient. Returns `false` when no
/// usable mailbox address is known.
fn wrong_recipient(to: &[String], cc: &[String], mailbox_address: Option<&str>) -> bool {
    let Some(mailbox) = mailbox_address.map(str::trim).filter(|m| m.contains('@')) else {
        return false;
    };
    let mailbox = mailbox.to_ascii_lowercase();
    !to.iter()
        .chain(cc.iter())
        .any(|a| a.trim().to_ascii_lowercase() == mailbox)
}

/// Bind a prose fact to the email envelope (issue #398):
///
/// 1. A fact without an explicit `valid_from` is anchored at the email's
///    sent date (falling back to the received date).
/// 2. An actionable fact (task / deadline / reminder) without an explicit
///    `valid_until` expires [`ACTIONABLE_WINDOW_DAYS`] after that anchor,
///    so urgency decays with the email's age and old mail can never
///    produce a current action item. Recurring facts are exempt — their
///    lifecycle is owned by the recurrence, not by a one-off window.
/// 3. Facts from forwarded or misdirected mail are never actionable —
///    `requires_user_action` is forced false and task-classified event
///    types (task / deadline / reminder) are cleared before the window
///    calculation, so they convey someone else's conversation and are
///    bounded to information only.
pub(crate) fn bind_prose_fact(fact: &mut NormalizedFact, envelope: &EmailEnvelope) {
    if envelope.is_forwarded || envelope.is_wrong_recipient {
        fact.requires_user_action = false;
        if is_task_classified(fact.event_type) {
            fact.event_type = None;
        }
    }
    if fact.valid_from.is_none() {
        fact.valid_from = envelope.sent_date.or(envelope.received_date);
    }
    let actionable = fact.requires_user_action || is_task_classified(fact.event_type);
    if actionable && fact.valid_until.is_none() && fact.recurrence == RecurrenceType::None {
        fact.valid_until = fact
            .valid_from
            .map(|from| from + Duration::days(ACTIONABLE_WINDOW_DAYS));
    }
}

/// Whether the fact carries a task-classified event type — the event kinds
/// that make a fact actionable and eligible for the one-off expiry window.
fn is_task_classified(event_type: Option<EventType>) -> bool {
    matches!(
        event_type,
        Some(EventType::Task | EventType::Deadline | EventType::Reminder)
    )
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
