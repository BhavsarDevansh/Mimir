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
    Arc::new(
        MockLlmClient::builder()
            .push_chat_message(llm_tool_message(json), Default::default())
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
async fn llm_layer_not_invoked_for_cancel_email() {
    // A CANCEL emits no facts but is still a handled iMIP part, so layer 3
    // must NOT run even when a backend is configured: cancellation prose
    // must never author junk facts (issue #283 cascade gate).
    let mock = llm_tool_response(r#"{"facts": []}"#);
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    stage(&connector, invite_email("CANCEL")).await;
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "CANCEL emits no facts: {facts:?}");
    assert!(
        mock.system_chat_calls().is_empty(),
        "LLM must not run on cancellation prose"
    );
    assert_eq!(
        connector.extract_deletions().await.expect("deletions"),
        vec!["dentist-1@example.com".to_string()],
        "the CANCEL tombstone is still buffered for the supervisor"
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
    stage(&connector, prose_email()).await;
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
    stage(&connector, prose_email()).await;
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "no deterministic facts for a prose email");
    assert_eq!(mock.system_chat_calls().len(), 1, "LLM was attempted once");
    assert!(
        !connector.buffer.lock().await.is_empty(),
        "failed raw email must be re-staged for retry"
    );
}

/// Build a plain-prose email fixture (no iMIP, no JSON-LD) that reaches the
/// LLM layer.
fn prose_email() -> Vec<u8> {
    b"From: reception@dentalclinic.com\r\n\
Subject: Your appointment\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
See you Tuesday 3pm. Please arrive 10 minutes early.\r\n"
        .to_vec()
}

fn llm_tool_message(json: &str) -> mimir_core::llm::Message {
    let tool_call = mimir_core::llm::ToolCall {
        index: 0,
        id: "call_1".into(),
        call_type: "function".into(),
        function: mimir_core::llm::FunctionCall {
            name: "extract_email_facts".into(),
            arguments: json.into(),
        },
    };
    mimir_core::llm::Message {
        role: "assistant".into(),
        content: String::new(),
        tool_calls: Some(vec![tool_call]),
        tool_call_id: None,
    }
}

#[tokio::test]
async fn llm_failure_is_bounded_and_terminates_after_max_attempts() {
    // A persistently failing message exhausts its bounded retry budget
    // (default 3 attempts, exponential cycle backoff): after the third
    // failure it is marked permanently failed, stops consuming LLM calls,
    // and is no longer re-staged.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .build(),
    );
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    stage(&connector, prose_email()).await;

    // Attempt 1 (cycle 1) fails → re-staged with a 1-cycle backoff.
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 1);
    // Cycle 2: backoff — no LLM call, message still staged.
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 1);
    assert_eq!(
        connector.buffer.lock().await.len(),
        1,
        "backoff keeps the message staged"
    );
    // Attempt 2 (cycle 3) fails → re-staged with a 2-cycle backoff.
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 2);
    // Cycles 4-5: backoff.
    connector.extract().await.expect("extract");
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 2);
    // Attempt 3 (cycle 6) fails → terminal failure: dropped, no re-stage.
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 3);
    assert!(
        connector.buffer.lock().await.is_empty(),
        "terminal failure must stop re-staging"
    );
    // Cycle 7: nothing staged, no further calls.
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 3);

    // The terminal failure is recorded durably and stops future attempts.
    let terminal = connector.prose_retry.lock().unwrap().terminal_count();
    assert_eq!(terminal, 1, "one terminal failure recorded");
    let durable = connector.durable_state().expect("dirty ledger persists");
    let restored = crate::email::llm::retry::ProseRetryLedger::from_json(&durable);
    assert_eq!(restored.terminal_count(), 1);
    assert!(
        restored.pending().next().is_none(),
        "no pending retries remain"
    );
}

#[tokio::test]
async fn llm_retry_succeeds_within_budget_and_clears_the_ledger() {
    // Two failures, then a success: the third attempt extracts the prose
    // facts and settles the ledger (no pending retry, no terminal record).
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_message(
                llm_tool_message(
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
                ),
                Default::default(),
            )
            .build(),
    );
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    stage(&connector, prose_email()).await;

    connector.extract().await.expect("extract"); // attempt 1 fails
    connector.extract().await.expect("extract"); // backoff
    connector.extract().await.expect("extract"); // attempt 2 fails
    connector.extract().await.expect("extract"); // backoff
    connector.extract().await.expect("extract"); // backoff
    let facts = connector.extract().await.expect("extract"); // attempt 3 succeeds
    assert_eq!(mock.system_chat_calls().len(), 3);
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].relationship_type, "has_appointment");
    assert_eq!(facts[0].raw_reference.as_deref(), Some("17:42"));
    assert!(
        connector.buffer.lock().await.is_empty(),
        "successful extraction must not re-stage"
    );
    let ledger = connector.prose_retry.lock().unwrap();
    assert_eq!(ledger.terminal_count(), 0);
    assert!(
        ledger.pending().next().is_none(),
        "ledger settled after success"
    );
}

#[tokio::test]
async fn restart_resume_re_stages_pending_retries_from_durable_state() {
    // Simulate a daemon restart: capture the durable ledger after one
    // failure, build a fresh connector with it injected (the supervisor
    // injects the persisted `__durable_state`), and verify the pending
    // message is re-staged and retried without an IMAP re-fetch.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_message(
                llm_tool_message(
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
                ),
                Default::default(),
            )
            .build(),
    );
    let connector = connector_with_llm(Some("Devansh"), Some(mock.clone()));
    stage(&connector, prose_email()).await;
    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 1);
    let durable = connector
        .durable_state()
        .expect("ledger is dirty after a failure");

    // Restart: a brand-new connector seeded from the persisted state.
    let mut config = app_config();
    config["__durable_state"] = serde_json::Value::String(durable);
    let restarted = EmailConnector::from_config_with_deps(
        config,
        None,
        Some("Devansh".to_string()),
        None,
        Some(mock.clone()),
    )
    .expect("config");
    assert_eq!(
        restarted.buffer.lock().await.len(),
        1,
        "pending retry must be re-staged from durable state"
    );

    // The next extraction cycle (after the backoff) retries attempt 2 and
    // succeeds against the re-staged raw bytes.
    restarted.extract().await.expect("extract"); // backoff cycle
    let facts = restarted.extract().await.expect("extract"); // attempt 2
    assert_eq!(mock.system_chat_calls().len(), 2);
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].raw_reference.as_deref(), Some("17:42"));
}

#[tokio::test]
async fn configurable_max_attempts_fails_terminal_immediately() {
    // `llm_extraction_max_attempts: 1` bounds the retry budget to a single
    // attempt: the first failure is terminal, so no LLM call is ever
    // repeated and nothing stays staged.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .build(),
    );
    let mut config = app_config();
    config["llm_extraction_max_attempts"] = serde_json::json!(1);
    let connector = EmailConnector::from_config_with_deps(
        config,
        None,
        Some("Devansh".to_string()),
        None,
        Some(mock.clone()),
    )
    .expect("config");
    stage(&connector, prose_email()).await;

    connector.extract().await.expect("extract");
    assert_eq!(mock.system_chat_calls().len(), 1);
    assert!(
        connector.buffer.lock().await.is_empty(),
        "terminal failure must stop re-staging"
    );
    connector.extract().await.expect("extract");
    assert_eq!(
        mock.system_chat_calls().len(),
        1,
        "no retry after a terminal failure"
    );
    assert_eq!(connector.prose_retry.lock().unwrap().terminal_count(), 1);
}
