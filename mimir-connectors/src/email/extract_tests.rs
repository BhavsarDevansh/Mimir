use super::*;

use crate::email::config::config_tests::app_config;
use crate::email::imap;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::EventType;
use mimir_knowledge::normalize::NormalizedFact;

pub(super) fn connector_with_identity(name: Option<&str>) -> EmailConnector {
    EmailConnector::from_config_with_deps(
        app_config(),
        None,
        name.map(|n| n.to_string()),
        None,
        None,
        None,
        None,
    )
    .expect("config")
}

/// Build a minimal RFC 822 email carrying one `text/calendar; method=<m>`
/// attachment whose body is a single VEVENT (a dentist appointment with a
/// location and two attendees). The plain-text body is included so the
/// calendar part is a real attachment, not the message body.
pub(super) fn invite_email(method: &str) -> Vec<u8> {
    invite_email_with_uid(method, Some("dentist-1@example.com"))
}

/// Like [`invite_email`] but omits the VEVENT `UID` property, so the
/// no-UID fallback paths (a REQUEST keyed by the email reference; a CANCEL
/// buffering no tombstone) can be exercised.
pub(super) fn invite_email_without_uid(method: &str) -> Vec<u8> {
    invite_email_with_uid(method, None)
}

fn invite_email_with_uid(method: &str, uid: Option<&str>) -> Vec<u8> {
    let uid_line = uid.map(|u| format!("UID:{u}\n")).unwrap_or_default();
    format!(
        r#"From: dentist@example.com
To: devansh@example.com
Subject: Dentist appointment
Date: Sat, 20 Nov 2025 14:22:01 -0800
Message-ID: <invite-1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="bnd"

--bnd
Content-Type: text/plain; charset="utf-8"

You are invited.
--bnd
Content-Type: text/calendar; method={method}; charset="utf-8"
Content-Disposition: attachment; filename="invite.ics"

BEGIN:VCALENDAR
VERSION:2.0
METHOD:{method}
BEGIN:VEVENT
{uid_line}SUMMARY:Dentist appointment
DTSTART:20991120T140000Z
DTEND:20991120T150000Z
LOCATION:123 Main St
ATTENDEE;CN=Devansh:mailto:devansh@example.com
ATTENDEE;CN=Dr Smith:mailto:smith@dental.com
END:VEVENT
END:VCALENDAR
--bnd--
"#,
        method = method,
        uid_line = uid_line
    )
    // RFC 5322/MIME require CRLF line endings and an IMAP `BODY.PEEK[]`
    // fetch returns CRLF, so normalise the bare-LF raw string to CRLF
    // rather than relying on the parser's leniency.
    .replace('\n', "\r\n")
    .into_bytes()
}

/// Like [`invite_email`] but lets the MIME `method` parameter and the
/// iCalendar body `METHOD` property diverge, so conflicting and
/// single-source combinations can be exercised independently.
fn invite_email_split_method(mime_method: Option<&str>, body_method: Option<&str>) -> Vec<u8> {
    let ct = match mime_method {
        Some(m) => format!("text/calendar; method={m}; charset=\"utf-8\""),
        None => "text/calendar; charset=\"utf-8\"".to_string(),
    };
    let cal_method_line = body_method
        .map(|m| format!("METHOD:{m}\n"))
        .unwrap_or_default();
    format!(
        r#"From: dentist@example.com
To: devansh@example.com
Subject: Dentist appointment
Date: Sat, 20 Nov 2025 14:22:01 -0800
Message-ID: <invite-1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="bnd"

--bnd
Content-Type: text/plain; charset="utf-8"

You are invited.
--bnd
Content-Type: {ct}
Content-Disposition: attachment; filename="invite.ics"

BEGIN:VCALENDAR
VERSION:2.0
{cal_method_line}BEGIN:VEVENT
UID:dentist-1@example.com
SUMMARY:Dentist appointment
DTSTART:20991120T140000Z
DTEND:20991120T150000Z
LOCATION:123 Main St
ATTENDEE;CN=Devansh:mailto:devansh@example.com
ATTENDEE;CN=Dr Smith:mailto:smith@dental.com
END:VEVENT
END:VCALENDAR
--bnd--
"#,
        ct = ct,
        cal_method_line = cal_method_line
    )
    .replace('\n', "\r\n")
    .into_bytes()
}

pub(super) fn plain_email() -> Vec<u8> {
    b"From: marketing@retailer.com\r\n\
To: devansh@example.com\r\n\
Subject: 20% off everything\r\n\
Date: Sat, 20 Nov 2025 14:22:01 -0800\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Sale! Sale! Sale!\r\n"
        .to_vec()
}

fn parse(bytes: &[u8]) -> mail_parser::Message<'_> {
    mail_parser::MessageParser::default()
        .parse(bytes)
        .expect("fixture must parse")
}

#[test]
fn extract_invites_emits_appointment_cluster_for_request_method() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email("REQUEST");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "42", &mut facts);
    assert!(handled, "REQUEST is a handled iMIP part");
    // 1 primary (has_event) + 1 location + 2 attendees = 4.
    assert_eq!(facts.len(), 4);
    let primary = facts
        .iter()
        .find(|f| f.relationship_type == "has_event")
        .unwrap();
    assert_eq!(primary.subject, "Devansh");
    assert_eq!(primary.subject_type, EntityType::Person);
    assert_eq!(primary.object, "Dentist appointment");
    assert_eq!(primary.object_type, Some(EntityType::Event));
    assert!(primary.valid_from.is_some());
    assert!(primary.valid_until.is_some());
    assert_eq!(primary.event_type, Some(EventType::Appointment));
    // Facts are keyed by the VEVENT UID namespaced as `imip:{uid}` — the
    // stable iMIP identity shared across REQUEST → REPLY → CANCEL, kept
    // disjoint from the email-UID space the JSON-LD / LLM layers write — so
    // a CANCEL can map onto them (issue #283).
    assert_eq!(
        primary.raw_reference.as_deref(),
        Some("imip:dentist-1@example.com")
    );
    // Location fact carries no temporal bounds.
    let loc = facts
        .iter()
        .find(|f| f.relationship_type == "located_in")
        .unwrap();
    assert_eq!(loc.object, "123 Main St");
    assert_eq!(loc.object_type, Some(EntityType::Place));
    assert!(loc.valid_from.is_none());
    assert!(loc.valid_until.is_none());
    // Two attendee facts, no temporal bounds.
    let attendees: Vec<&NormalizedFact> = facts
        .iter()
        .filter(|f| f.relationship_type == "attending")
        .collect();
    assert_eq!(attendees.len(), 2);
    assert!(attendees.iter().all(|a| a.valid_from.is_none()));
    assert_eq!(attendees[0].subject, "Devansh");
    assert_eq!(attendees[1].subject, "Dr Smith");
}

#[test]
fn extract_invites_emits_facts_for_reply_method() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email("REPLY");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "7", &mut facts);
    assert!(handled, "REPLY is a handled iMIP part");
    // Same cluster shape as REQUEST.
    assert!(facts.iter().any(|f| f.relationship_type == "has_event"));
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.relationship_type == "attending")
            .count(),
        2
    );
    assert!(
        facts
            .iter()
            .all(|f| f.raw_reference.as_deref() == Some("imip:dentist-1@example.com")),
        "REPLY facts are keyed by the namespaced VEVENT UID: {facts:?}"
    );
}

#[test]
fn extract_invites_skips_publish_method() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email("PUBLISH");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "9", &mut facts);
    assert!(
        facts.is_empty(),
        "PUBLISH (often marketing webinars) is skipped for now"
    );
    assert!(!handled, "PUBLISH is not a handled iMIP part");
}

#[tokio::test]
async fn extract_invites_buffers_cancel_uid_as_tombstone() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email("CANCEL");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "9", &mut facts);
    assert!(facts.is_empty(), "CANCEL emits no facts");
    assert!(handled, "CANCEL counts as a read iMIP part (cascade gate)");
    assert_eq!(
        connector.extract_deletions().await.expect("deletions"),
        vec!["imip:dentist-1@example.com".to_string()],
        "the CANCEL VEVENT UID is buffered as a namespaced tombstone"
    );
}

#[tokio::test]
async fn extract_cancel_in_same_batch_drops_request_facts_for_the_uid() {
    let connector = connector_with_identity(Some("Devansh"));
    // A REQUEST and its CANCEL for the same VEVENT UID arrive in one sync
    // batch (both fetched since the last cursor). The supervisor trashes
    // *before* inserting this cycle's facts, so the CANCEL — the later
    // signal — must also drop the batch's fresh REQUEST facts, or the
    // cancelled event would be inserted after the trash and survive.
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 1,
        internal_date: None,
        raw: invite_email("REQUEST"),
    });
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 43,
        uid_validity: 1,
        internal_date: None,
        raw: invite_email("CANCEL"),
    });
    let facts = connector.extract().await.expect("extract");
    assert!(
        facts.is_empty(),
        "the CANCEL (later signal) must win over the same-batch REQUEST: {facts:?}"
    );
    assert_eq!(
        connector.extract_deletions().await.expect("deletions"),
        vec!["imip:dentist-1@example.com".to_string()],
        "the CANCEL tombstone is still reported for the supervisor's trash pass"
    );
}

#[tokio::test]
async fn extract_cancel_before_request_in_same_batch_drops_request_facts() {
    let connector = connector_with_identity(Some("Devansh"));
    // A CANCEL staged *before* its REQUEST in the same sync batch: buffer
    // order is not guaranteed to match iMIP order (a failed cycle's re-fetch
    // appends its window to the tail, and mail delivery can invert order).
    // The tombstone filter must run after the whole message loop, not only
    // at CANCEL time, or the REQUEST facts would be inserted after the
    // supervisor's trash pass and survive.
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 1,
        internal_date: None,
        raw: invite_email("CANCEL"),
    });
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 43,
        uid_validity: 1,
        internal_date: None,
        raw: invite_email("REQUEST"),
    });
    let facts = connector.extract().await.expect("extract");
    assert!(
        facts.is_empty(),
        "the CANCEL must win even when staged before its REQUEST: {facts:?}"
    );
    assert_eq!(
        connector.extract_deletions().await.expect("deletions"),
        vec!["imip:dentist-1@example.com".to_string()],
        "the CANCEL tombstone is still reported for the supervisor's trash pass"
    );
}

#[tokio::test]
async fn cancel_tombstones_survive_restart_via_durable_state() {
    // A CANCEL is consumed by `extract` and the IMAP cursor has already
    // advanced past it, so if the daemon stops between `extract` and the
    // supervisor's deletion pass the tombstone must be restored from the
    // persisted durable state or the cancellation is lost permanently.
    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 1,
        internal_date: None,
        raw: invite_email("CANCEL"),
    });
    connector.extract().await.expect("extract");
    let durable = connector
        .durable_state()
        .expect("the buffered tombstone makes the durable state dirty");

    // Restart: a fresh connector seeded from the persisted state re-reports
    // the pending tombstone so the supervisor's trash pass is not lost.
    let mut config = app_config();
    config["__durable_state"] = serde_json::Value::String(durable);
    let restarted = EmailConnector::from_config_with_deps(
        config,
        None,
        Some("Devansh".to_string()),
        None,
        None,
        None,
        None,
    )
    .expect("config");
    assert_eq!(
        restarted.extract_deletions().await.expect("deletions"),
        vec!["imip:dentist-1@example.com".to_string()],
        "a restart between extract and the deletion pass re-reports the CANCEL"
    );
}

#[tokio::test]
async fn extract_invites_cancel_without_uid_buffers_nothing() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email_without_uid("CANCEL");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "9", &mut facts);
    assert!(facts.is_empty(), "CANCEL emits no facts");
    assert!(handled, "the CANCEL part is still read");
    assert!(
        connector
            .extract_deletions()
            .await
            .expect("deletions")
            .is_empty(),
        "a CANCEL VEVENT without a UID cannot be mapped to prior facts"
    );
}

#[test]
fn extract_invites_falls_back_to_email_ref_when_vevent_has_no_uid() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email_without_uid("REQUEST");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "42", &mut facts);
    assert!(handled, "REQUEST is a handled iMIP part");
    assert!(
        facts.iter().any(|f| f.relationship_type == "has_event"),
        "a UID-less VEVENT still extracts (SUMMARY names the event)"
    );
    assert!(
        facts
            .iter()
            .all(|f| f.raw_reference.as_deref() == Some("imip-email:42")),
        "a UID-less VEVENT falls back to the namespaced email reference: {facts:?}"
    );
}

#[test]
fn extract_invites_skips_conflicting_mime_and_body_method() {
    let connector = connector_with_identity(Some("Devansh"));
    // MIME says REQUEST, body says CANCEL — must be rejected, not silently
    // honoured as REQUEST (which would create appointment facts).
    let bytes = invite_email_split_method(Some("REQUEST"), Some("CANCEL"));
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "9", &mut facts);
    assert!(
        facts.is_empty(),
        "a part whose MIME `method` and body `METHOD` disagree is not a valid iMIP object"
    );
    assert!(!handled, "a conflicting-METHOD part is not handled");
}

#[test]
fn extract_invites_skips_conflicting_supported_and_unsupported_method() {
    let connector = connector_with_identity(Some("Devansh"));
    // MIME says REPLY (supported), body says PUBLISH (unsupported) — the
    // conflict is rejected before the supported/unsupported filter runs.
    let bytes = invite_email_split_method(Some("REPLY"), Some("PUBLISH"));
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "9", &mut facts);
    assert!(
        facts.is_empty(),
        "conflicting METHOD values are rejected regardless of which side is supported"
    );
    assert!(!handled, "a conflicting-METHOD part is not handled");
}

#[test]
fn extract_invites_falls_back_to_body_method_when_mime_absent() {
    let connector = connector_with_identity(Some("Devansh"));
    // No MIME `method` parameter; the body `METHOD:REQUEST` is the source.
    let bytes = invite_email_split_method(None, Some("REQUEST"));
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "7", &mut facts);
    assert!(handled, "the body METHOD is honoured");
    assert!(
        facts.iter().any(|f| f.relationship_type == "has_event"),
        "the iCalendar body `METHOD` property is used when the MIME parameter is absent"
    );
}

#[test]
fn extract_invites_skips_when_neither_method_source_present() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = invite_email_split_method(None, None);
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "9", &mut facts);
    assert!(
        facts.is_empty(),
        "no METHOD from either source → unsupported/absent → skipped"
    );
    assert!(!handled, "no METHOD means nothing was handled");
}

#[test]
fn extract_invites_skips_plain_email() {
    let connector = connector_with_identity(Some("Devansh"));
    let bytes = plain_email();
    let message = parse(&bytes);
    // No text/calendar part → no facts. A marketing email produces nothing.
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "1", &mut facts);
    assert!(facts.is_empty());
    assert!(!handled, "a plain email has no iMIP part");
}

#[test]
fn extract_invites_skips_primary_when_no_user_identity() {
    let connector = connector_with_identity(None);
    let bytes = invite_email("REQUEST");
    let message = parse(&bytes);
    let mut facts = Vec::new();
    let handled = connector.extract_invites(&message, "42", &mut facts);
    assert!(handled);
    // No user identity → no primary has_event; the event is still captured
    // via its location + attendee facts.
    assert!(facts.iter().all(|f| f.relationship_type != "has_event"));
    assert!(facts.iter().any(|f| f.relationship_type == "located_in"));
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.relationship_type == "attending")
            .count(),
        2
    );
}

#[tokio::test]
async fn extract_drains_buffer_and_returns_invite_facts() {
    let connector = connector_with_identity(Some("Devansh"));
    // Stage a raw invite email in the buffer (as a sync cycle would).
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 1,
        internal_date: None,
        raw: invite_email("REQUEST"),
    });
    let facts = connector.extract().await.expect("extract");
    // The cascade produced the appointment cluster; the buffer is drained.
    assert!(facts.iter().any(|f| f.relationship_type == "has_event"));
    assert!(
        connector.buffer.lock().await.is_empty(),
        "buffer must be drained"
    );
}

#[tokio::test]
async fn extract_with_no_invite_emails_yields_no_facts() {
    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 1,
        uid_validity: 1,
        internal_date: None,
        raw: plain_email(),
    });
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "plain/marketing email → no facts");
}

// --- #249: schema.org JSON-LD extraction (cascade layer 2) ---------------
/// Build a minimal RFC 822 email carrying an HTML body with one
/// `<script type="application/ld+json">` block containing a
/// `FlightReservation`. The plain-text alternative is included so the
/// HTML part is a real alternative body part, not the sole body.
pub(super) fn jsonld_flight_email() -> Vec<u8> {
    r#"From: noreply@airline.com
To: devansh@example.com
Subject: Flight confirmation BA123
Date: Mon, 04 Aug 2025 10:00:00 +0000
Message-ID: <flight-1@airline.com>
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="bnd"

--bnd
Content-Type: text/plain; charset="utf-8"

Your flight is confirmed.
--bnd
Content-Type: text/html; charset="utf-8"

<html><body>
<h1>Flight confirmation</h1>
<script type="application/ld+json">{
  "@context": "https://schema.org",
  "@type": "FlightReservation",
  "reservationId": "ABC123",
  "passengerName": "Devansh Bhavsar",
  "reservationFor": {
    "@type": "Flight",
    "flightNumber": "123",
    "airline": { "@type": "Airline", "name": "British Airways" },
    "departureAirport": { "@type": "Airport", "name": "Heathrow Airport", "iataCode": "LHR" },
    "departureTime": "2099-08-15T10:00:00+01:00",
    "arrivalAirport": { "@type": "Airport", "name": "Fiumicino Airport", "iataCode": "FCO" },
    "arrivalTime": "2099-08-15T13:30:00+02:00"
  }
}</script>
</body></html>
--bnd--
"#
    .replace('\n', "\r\n")
    .into_bytes()
}

/// An email with an HTML body containing an unrecognised JSON-LD type
/// (`Person`) — should produce no facts.
pub(super) fn jsonld_unrecognised_email() -> Vec<u8> {
    r#"From: noreply@example.com
To: devansh@example.com
Subject: Hello
Date: Mon, 04 Aug 2025 10:00:00 +0000
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="bnd"

--bnd
Content-Type: text/plain; charset="utf-8"

Hello.
--bnd
Content-Type: text/html; charset="utf-8"

<html><body>
<script type="application/ld+json">{ "@type": "Person", "name": "Someone" }</script>
</body></html>
--bnd--
"#
    .replace('\n', "\r\n")
    .into_bytes()
}

#[tokio::test]
async fn extract_jsonld_email_produces_flight_facts() {
    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 99,
        uid_validity: 17,
        internal_date: None,
        raw: jsonld_flight_email(),
    });
    let facts = connector.extract().await.expect("extract");
    // 1 has_flight + 1 departs_from + 1 arrives_at + 1 operated_by = 4
    assert!(
        facts.iter().any(|f| f.relationship_type == "has_flight"),
        "JSON-LD flight facts extracted: {:?}",
        facts
    );
    assert_eq!(facts.len(), 4);
    // Provenance: raw_reference is the UIDVALIDITY-qualified UID.
    assert!(
        facts
            .iter()
            .all(|f| f.raw_reference.as_deref() == Some("17:99")),
        "raw_reference must be UIDVALIDITY-qualified UID: {:?}",
        facts
    );
    // Buffer is drained.
    assert!(connector.buffer.lock().await.is_empty());
}

#[tokio::test]
async fn extract_jsonld_unrecognised_type_produces_no_facts() {
    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 1,
        uid_validity: 1,
        internal_date: None,
        raw: jsonld_unrecognised_email(),
    });
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "unrecognised JSON-LD type → no facts");
}

#[tokio::test]
async fn extract_cascade_runs_both_imip_and_jsonld_layers() {
    // An email with both a text/calendar invite AND a JSON-LD
    // EventReservation in the HTML body — both layers should produce facts.
    let combined = r#"From: noreply@example.com
To: devansh@example.com
Subject: Event + invite
Date: Mon, 04 Aug 2025 10:00:00 +0000
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="outer"

--outer
Content-Type: text/html; charset="utf-8"

<html><body>
<script type="application/ld+json">{ "@type": "EventReservation", "reservationFor": { "@type": "Event", "name": "JSON-LD Event", "startDate": "2025-09-10T19:30:00Z" } }</script>
</body></html>
--outer
Content-Type: text/calendar; method=REQUEST; charset="utf-8"
Content-Disposition: attachment; filename="invite.ics"

BEGIN:VCALENDAR
VERSION:2.0
METHOD:REQUEST
BEGIN:VEVENT
UID:ical-1@example.com
SUMMARY:iMIP Event
DTSTART:20991120T140000Z
DTEND:20991120T150000Z
END:VEVENT
END:VCALENDAR
--outer--
"#
        .replace('\n', "\r\n")
        .into_bytes();

    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 50,
        uid_validity: 3,
        internal_date: None,
        raw: combined,
    });
    let facts = connector.extract().await.expect("extract");
    // Layer 1 (iMIP): 1 has_event for "iMIP Event"
    // Layer 2 (JSON-LD): 1 has_event for "JSON-LD Event" + 0 location (none in fixture)
    let has_event_count = facts
        .iter()
        .filter(|f| f.relationship_type == "has_event")
        .count();
    assert_eq!(
        has_event_count, 2,
        "both cascade layers should emit has_event: {:?}",
        facts
    );
    assert!(
        facts.iter().any(|f| f.object == "iMIP Event"),
        "layer 1 iMIP fact present"
    );
    assert!(
        facts.iter().any(|f| f.object == "JSON-LD Event"),
        "layer 2 JSON-LD fact present"
    );
}
