//! Envelope derivation, classification, and temporal-binding tests
//! (issue #398).

use super::*;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use mail_parser::MessageParser;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

/// Parse RFC 822 text (normalised to CRLF, like an IMAP fetch returns) into
/// a message borrowing the caller-owned buffer.
fn parse(raw: &str) -> mail_parser::Message<'_> {
    MessageParser::default()
        .parse(raw.as_bytes())
        .expect("parses")
}

/// Normalise a fixture's bare-LF headers to CRLF (an IMAP `BODY.PEEK[]`
/// fetch returns CRLF).
fn crlf(raw: String) -> String {
    raw.replace('\n', "\r\n")
}

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, s).single().unwrap()
}

/// A bare fact with the connector defaults, for binding tests.
fn prose_fact(requires_user_action: bool) -> NormalizedFact {
    NormalizedFact {
        confidence: None,
        source_type: SourceType::Connector,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "has_task".to_string(),
        object: "pay rent".to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: mimir_knowledge::models::enums::RecurrenceType::None,
        requires_user_action,
        raw_reference: Some("17:42".to_string()),
        extraction_method: Some(ExtractionMethod::LlmExtraction),
        event_type: None,
        location: None,
    }
}

fn base_email() -> String {
    "From: landlord@example.com
To: devansh@example.com
Cc: accountant@example.com
Reply-To: property@example.com
Subject: Rent reminder
Date: Wed, 20 Aug 2025 09:00:00 +0000

Please pay rent by Friday.
"
    .to_string()
}

#[test]
fn envelope_carries_dates_sender_and_recipients() {
    let raw = base_email().replace(
        "Subject: Rent reminder\n",
        "Subject: Rent reminder\nList-Unsubscribe: <https://unsub.example.com>\n",
    );
    let raw = crlf(raw);
    let message = parse(&raw);
    let internal = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2025, 8, 20, 9, 5, 0)
        .unwrap();
    let envelope =
        EmailEnvelope::from_message(&message, Some(internal), Some("devansh@example.com"));

    assert_eq!(envelope.sent_date, Some(utc(2025, 8, 20, 9, 0, 0)));
    assert_eq!(envelope.received_date, Some(utc(2025, 8, 20, 9, 5, 0)));
    assert_eq!(envelope.from.as_deref(), Some("landlord@example.com"));
    assert_eq!(envelope.to, vec!["devansh@example.com".to_string()]);
    assert_eq!(envelope.cc, vec!["accountant@example.com".to_string()]);
    assert_eq!(envelope.reply_to, vec!["property@example.com".to_string()]);
    assert_eq!(envelope.subject, "Rent reminder");
    assert!(envelope.has_list_unsubscribe);
    assert!(envelope.is_spam, "List-Unsubscribe marks bulk mail");
    assert!(!envelope.is_forwarded);
    assert!(!envelope.is_wrong_recipient, "owner is in To");
}

#[test]
fn envelope_carries_the_body_text_for_prompt_reuse() {
    // The envelope's body extraction is shared with the LLM prompt, so the
    // prose layer never runs the body pass twice (issue #398 review).
    let raw = crlf(base_email());
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, None);
    assert_eq!(
        envelope.body.as_deref().map(str::trim),
        Some("Please pay rent by Friday."),
        "the envelope reuses its body extraction for the LLM prompt"
    );

    // A subject-marked forward skips the body pass (the subject already
    // classifies it), so the envelope body is `None` and the LLM layer
    // extracts the body on demand for the prompt.
    let raw = base_email().replacen("Subject: Rent reminder", "Subject: Fwd: Rent reminder", 1);
    let raw = crlf(raw);
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, None);
    assert!(envelope.is_forwarded);
    assert_eq!(envelope.body, None);
}

#[test]
fn wrong_recipient_detected_when_mailbox_absent_from_to_and_cc() {
    let raw = base_email().replace("devansh@example.com", "other@example.com");
    let raw = crlf(raw);
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, Some("devansh@example.com"));
    assert!(envelope.is_wrong_recipient);
}

#[test]
fn not_wrong_recipient_when_mailbox_in_cc() {
    let raw = base_email().replace("Cc: accountant@example.com", "Cc: devansh@example.com");
    let raw = crlf(raw);
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, Some("devansh@example.com"));
    assert!(!envelope.is_wrong_recipient, "owner is in Cc");
}

#[test]
fn not_wrong_recipient_without_usable_mailbox_address() {
    let raw = base_email().replace("devansh@example.com", "other@example.com");
    let raw = crlf(raw);
    let message = parse(&raw);
    // No mailbox address known → never flagged.
    assert!(!EmailEnvelope::from_message(&message, None, None).is_wrong_recipient);
    // A non-email IMAP username cannot match a recipient either.
    assert!(!EmailEnvelope::from_message(&message, None, Some("devansh")).is_wrong_recipient);
}

#[test]
fn forwarded_detected_from_subject_prefixes() {
    for subject in [
        "Fwd: rent reminder",
        "FW: rent reminder",
        "fwd: rent reminder",
    ] {
        let raw =
            base_email().replacen("Subject: Rent reminder", &format!("Subject: {subject}"), 1);
        let raw = crlf(raw);
        let message = parse(&raw);
        assert!(
            EmailEnvelope::from_message(&message, None, None).is_forwarded,
            "subject {subject:?} must be forwarded"
        );
    }
    let raw = crlf(base_email());
    let message = parse(&raw);
    assert!(!EmailEnvelope::from_message(&message, None, None).is_forwarded);
}

#[test]
fn forwarded_detected_from_body_separator() {
    let raw = format!(
        "{}\n---------- Forwarded message ----------\noriginal body\n",
        base_email()
    );
    let raw = crlf(raw);
    let message = parse(&raw);
    assert!(EmailEnvelope::from_message(&message, None, None).is_forwarded);
}

#[test]
fn spam_gate_flags_unsubscribe_and_marketing_domains() {
    let raw = base_email().replace("landlord@example.com", "promo@mailchimp.com");
    let raw = crlf(raw);
    let message = parse(&raw);
    assert!(EmailEnvelope::from_message(&message, None, None).is_spam);

    // A general-purpose ESP without a bulk signal is not spam by domain.
    let raw = base_email().replace("landlord@example.com", "receipt@sendgrid.net");
    let raw = crlf(raw);
    let message = parse(&raw);
    assert!(!EmailEnvelope::from_message(&message, None, None).is_spam);
}

#[test]
fn bind_prose_fact_derives_temporals_from_envelope() {
    let raw = crlf(base_email());
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, Some("devansh@example.com"));
    let mut fact = prose_fact(true);
    bind_prose_fact(&mut fact, &envelope);
    assert_eq!(fact.valid_from, Some(utc(2025, 8, 20, 9, 0, 0)));
    assert_eq!(
        fact.valid_until,
        Some(utc(2025, 9, 19, 9, 0, 0)),
        "actionable facts expire 30 days after the email date"
    );
    assert!(fact.requires_user_action);
}

#[test]
fn bind_prose_fact_falls_back_to_received_date_without_date_header() {
    let raw = base_email()
        .lines()
        .filter(|l| !l.starts_with("Date:"))
        .collect::<Vec<_>>()
        .join("\n");
    let raw = crlf(raw);
    let message = parse(&raw);
    let internal = FixedOffset::east_opt(5 * 3600)
        .unwrap()
        .with_ymd_and_hms(2025, 8, 20, 14, 0, 0)
        .unwrap();
    let envelope = EmailEnvelope::from_message(&message, Some(internal), None);
    let mut fact = prose_fact(true);
    bind_prose_fact(&mut fact, &envelope);
    assert_eq!(
        fact.valid_from,
        Some(utc(2025, 8, 20, 9, 0, 0)),
        "INTERNALDATE converts to UTC"
    );
}

#[test]
fn bind_prose_fact_preserves_explicit_temporal_bounds() {
    let raw = crlf(base_email());
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, None);
    let mut fact = prose_fact(true);
    fact.valid_from = Some(utc(2026, 1, 1, 10, 0, 0));
    fact.valid_until = Some(utc(2026, 1, 2, 10, 0, 0));
    bind_prose_fact(&mut fact, &envelope);
    assert_eq!(fact.valid_from, Some(utc(2026, 1, 1, 10, 0, 0)));
    assert_eq!(fact.valid_until, Some(utc(2026, 1, 2, 10, 0, 0)));
}

#[test]
fn bind_prose_fact_keeps_non_actionable_facts_open_ended() {
    let raw = crlf(base_email());
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, None);
    let mut fact = prose_fact(false);
    bind_prose_fact(&mut fact, &envelope);
    assert_eq!(fact.valid_from, Some(utc(2025, 8, 20, 9, 0, 0)));
    assert_eq!(
        fact.valid_until, None,
        "non-actionable facts stay open-ended"
    );
}

#[test]
fn bind_prose_fact_leaves_recurring_facts_to_their_recurrence() {
    // A recurring obligation (e.g. "pay rent monthly") owns its own
    // lifecycle, so the one-off actionable window must not truncate it.
    let raw = crlf(base_email());
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, None);
    let mut fact = prose_fact(true);
    fact.recurrence = mimir_knowledge::models::enums::RecurrenceType::Monthly;
    bind_prose_fact(&mut fact, &envelope);
    assert_eq!(fact.valid_from, Some(utc(2025, 8, 20, 9, 0, 0)));
    assert_eq!(
        fact.valid_until, None,
        "recurrence owns the lifecycle, not the actionable window"
    );
}

#[test]
fn bind_prose_fact_forwarded_and_wrong_recipient_are_not_actionable() {
    let raw = base_email().replacen("To: devansh@example.com", "To: other@example.com", 1);
    let raw = crlf(raw);
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, Some("devansh@example.com"));
    assert!(envelope.is_wrong_recipient);
    for event_type in [EventType::Task, EventType::Deadline, EventType::Reminder] {
        let mut fact = prose_fact(true);
        fact.event_type = Some(event_type);
        bind_prose_fact(&mut fact, &envelope);
        assert!(
            !fact.requires_user_action,
            "misdirected mail is never actionable"
        );
        assert_eq!(
            fact.event_type, None,
            "task classification cannot survive the downgrade"
        );
        assert_eq!(
            fact.valid_until, None,
            "misdirected mail gains no actionable expiry window"
        );
    }

    let raw = base_email().replacen("Subject: Rent reminder", "Subject: Fwd: Rent reminder", 1);
    let raw = crlf(raw);
    let message = parse(&raw);
    let envelope = EmailEnvelope::from_message(&message, None, None);
    assert!(envelope.is_forwarded);
    for event_type in [EventType::Task, EventType::Deadline, EventType::Reminder] {
        let mut fact = prose_fact(true);
        fact.event_type = Some(event_type);
        bind_prose_fact(&mut fact, &envelope);
        assert!(
            !fact.requires_user_action,
            "forwarded mail is never actionable"
        );
        assert_eq!(
            fact.event_type, None,
            "task classification cannot survive the downgrade"
        );
        assert_eq!(
            fact.valid_until, None,
            "forwarded mail gains no actionable expiry window"
        );
    }
}
