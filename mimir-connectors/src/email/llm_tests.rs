use super::*;

use crate::email::config::config_tests::app_config;
use crate::email::imap;
use mimir_core::llm::MockLlmClient;

use super::extract_tests::{invite_email, plain_email};

/// Construct a connector with an injected LLM backend (and the user
/// identity) so the layer-3 prose path is exercised end-to-end through
/// `extract()`.
fn connector_with_llm(name: Option<&str>, backend: Option<Arc<dyn LlmBackend>>) -> EmailConnector {
    EmailConnector::from_config_with_deps(
        app_config(),
        None,
        name.map(|n| n.to_string()),
        None,
        backend,
    )
    .expect("config")
}

fn llm_tool_response(json: &str) -> Arc<MockLlmClient> {
    let tool_call = mimir_core::llm::ToolCall {
        index: 0,
        id: "call_1".into(),
        call_type: "function".into(),
        function: mimir_core::llm::FunctionCall {
            name: "extract_email_facts".into(),
            arguments: json.into(),
        },
    };
    let message = mimir_core::llm::Message {
        role: "assistant".into(),
        content: String::new(),
        tool_calls: Some(vec![tool_call]),
        tool_call_id: None,
    };
    Arc::new(
        MockLlmClient::builder()
            .push_chat_message(message, Default::default())
            .build(),
    )
}

async fn stage(connector: &EmailConnector, raw: Vec<u8>) {
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 17,
        internal_date: None,
        raw,
    });
}

#[tokio::test]
async fn llm_layer_skipped_when_no_backend_configured() {
    // With no backend, a plain-prose email produces no facts: the
    // deterministic layers read nothing and layer 3 is disabled.
    let connector = connector_with_llm(Some("Devansh"), None);
    stage(&connector, plain_email()).await;
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "no backend -> no LLM facts");
}

#[tokio::test]
async fn llm_layer_not_invoked_when_deterministic_layer_already_read_the_email() {
    // An iMIP invite yields deterministic facts, so layer 3 must NOT run
    // even when a backend is configured (cascade gate avoids duplicate
    // extraction and an unnecessary LLM call).
    let mock = llm_tool_response(r#"{"facts": []}"#);
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    stage(&connector, invite_email("REQUEST")).await;
    let facts = connector.extract().await.expect("extract");
    assert!(
        facts.iter().any(|f| f.relationship_type == "has_event"),
        "deterministic invite facts still extracted: {facts:?}"
    );
    assert!(
        mock.system_chat_calls().is_empty(),
        "LLM must not run when a deterministic layer already produced facts"
    );
}

#[tokio::test]
async fn llm_layer_extracts_prose_when_no_deterministic_facts() {
    // A plain-prose appointment email yields nothing from layers 1-2, so
    // layer 3 runs and the LLM's validated facts are appended with
    // `extraction_method = LlmExtraction`.
    let mock = llm_tool_response(
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
    );
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    let prose: Vec<u8> = b"From: reception@dentalclinic.com\r\n\
Subject: Your appointment\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
See you Tuesday 3pm. Please arrive 10 minutes early.\r\n"
        .to_vec();
    stage(&connector, prose).await;
    let facts = connector.extract().await.expect("extract");
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].relationship_type, "has_appointment");
    assert_eq!(facts[0].subject, "Devansh");
    assert_eq!(
        facts[0].extraction_method,
        Some(mimir_knowledge::models::source::ExtractionMethod::LlmExtraction)
    );
    assert_eq!(facts[0].raw_reference.as_deref(), Some("17:42"));
    assert_eq!(mock.system_chat_calls().len(), 1);
}

#[tokio::test]
async fn llm_layer_spam_email_skips_call_and_yields_no_facts() {
    // An obvious bulk-marketing email is skipped by the Rust pre-filter
    // before any LLM call: no facts, no system-queue call.
    let mock = llm_tool_response(r#"{"facts": []}"#);
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    let spam: Vec<u8> = b"From: promo@mailchimp.com\r\n\
Subject: 50% off everything\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Sale ends Sunday!\r\n"
        .to_vec();
    stage(&connector, spam).await;
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty());
    assert!(mock.system_chat_calls().is_empty());
}

#[tokio::test]
async fn llm_failure_re_stages_raw_email_for_retry() {
    // A retryable LLM failure must not become a silent empty extraction:
    // the raw email is re-staged in the buffer so the next extraction
    // cycle retries it, and the deterministic layers' facts (here none)
    // are returned without aborting the batch.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .build(),
    );
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    let prose: Vec<u8> = b"From: reception@dentalclinic.com\r\n\
Subject: Your appointment\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
See you Tuesday 3pm.\r\n"
        .to_vec();
    stage(&connector, prose).await;
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "no deterministic facts for a prose email");
    assert_eq!(mock.system_chat_calls().len(), 1, "LLM was attempted once");
    assert!(
        !connector.buffer.lock().await.is_empty(),
        "failed raw email must be re-staged for retry"
    );
}
