use super::*;

use super::message::{canonicalise_subject, is_likely_spam, strip_html};
use mimir_core::llm::MockLlmClient;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

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
