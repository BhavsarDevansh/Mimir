use super::*;

use std::future::Future;
use std::time::Duration;

use crate::email::config::config_tests::app_config;
use crate::email::imap;
use crate::email::llm::{EmailExtractionHook, extract_prose_facts};
use chrono::TimeZone;
use mimir_core::hooks::{
    Gate, Hook, HookEngine, KeyScope, QueuePolicy, RetryPolicy, Trigger, TriggerKind,
};
use mimir_core::job_queue::JobQueue;
use mimir_core::llm::MockLlmClient;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};
use mimir_knowledge::models::fact::Fact;

use super::extract_tests::{invite_email, plain_email};

/// Construct a connector with an injected LLM backend (and the user
/// identity) so the layer-3 prose path is exercised end-to-end through
/// `extract()`.
fn connector_with_llm(name: Option<&str>, backend: Option<Arc<dyn LlmBackend>>) -> EmailConnector {
    EmailConnector::from_config_with_deps(
        app_config(),
        EmailConnectorDeps {
            user_identity: name.map(|n| n.to_string()),
            llm_backend: backend,
            ..Default::default()
        },
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

fn taxonomy_names() -> Vec<String> {
    mimir_knowledge::CANONICAL_PREDICATES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

async fn stage(connector: &EmailConnector, raw: Vec<u8>) {
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 17,
        internal_date: None,
        raw,
    });
}

/// A connector wired with the shared knowledge graph and a running hooks
/// engine (issue #386): `extract()` enqueues prose emails as
/// `connector_item.remember` instances and the dispatch loop runs the
/// extraction handler.
struct HookEnv {
    connector: EmailConnector,
    kg: Arc<KnowledgeGraph>,
    engine: Arc<HookEngine>,
    _kg_dir: tempfile::TempDir,
    _jobs_dir: tempfile::TempDir,
    _loop: tokio::task::JoinHandle<()>,
}

async fn hook_env(backend: Option<Arc<dyn LlmBackend>>, max_attempts: Option<u8>) -> HookEnv {
    hook_env_with_policy(backend, max_attempts, None).await
}

async fn hook_env_with_policy(
    backend: Option<Arc<dyn LlmBackend>>,
    max_attempts: Option<u8>,
    max_pending: Option<usize>,
) -> HookEnv {
    let kg_dir = tempfile::tempdir().unwrap();
    let kg = Arc::new(
        KnowledgeGraph::init(&kg_dir.path().join("knowledge.db"))
            .await
            .unwrap(),
    );
    // Register a Gmail connector row so connector provenance has a valid
    // `connector_instance_id` FK.
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Email,
            slug: "gmail-test".to_string(),
            backend: "imap".to_string(),
            display_name: "Gmail".to_string(),
            config_json: "{}".to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();

    let jobs_dir = tempfile::tempdir().unwrap();
    let jq = Arc::new(
        JobQueue::init(jobs_dir.path().join("jobs.db"))
            .await
            .unwrap(),
    );
    let llm = backend
        .clone()
        .unwrap_or_else(|| Arc::new(MockLlmClient::builder().build()));
    let (engine, shutdown_rx) = HookEngine::new(jq, llm);
    engine
        .register(Hook {
            id: "connector_item.remember".to_string(),
            trigger: TriggerKind::ConnectorItemStaged,
            key_scope: KeyScope::PerKey,
            policy: QueuePolicy::Multiple,
            gate: Gate::Ungated,
            retry: RetryPolicy {
                max_attempts: u8::MAX,
                backoff: Duration::from_millis(10),
            },
            max_pending,
            merge: None,
            handler: Arc::new(EmailExtractionHook::new()),
        })
        .await
        .unwrap();
    let engine_clone = Arc::clone(&engine);
    let loop_handle = tokio::spawn(async move { engine_clone.start(shutdown_rx).await });

    let mut config = app_config();
    config["__instance_id"] = serde_json::json!(row.id);
    if let Some(max) = max_attempts {
        config["llm_extraction_max_attempts"] = serde_json::json!(max);
    }
    let connector = EmailConnector::from_config_with_deps(
        config,
        EmailConnectorDeps {
            user_identity: Some("Devansh".to_string()),
            llm_backend: backend,
            kg: Some(Arc::clone(&kg)),
            hook_engine: Some(Arc::clone(&engine)),
            ..Default::default()
        },
    )
    .expect("config");

    HookEnv {
        connector,
        kg,
        engine,
        _kg_dir: kg_dir,
        _jobs_dir: jobs_dir,
        _loop: loop_handle,
    }
}

/// Whether the KG holds a fact for the user with the given predicate and
/// object (entity name or literal).
async fn has_fact(kg: &KnowledgeGraph, predicate: &str, object: &str) -> bool {
    let search = kg.search_entities("Devansh", 10).await.unwrap();
    for result in &search {
        let facts = kg
            .get_facts_by_subject(result.entity.id, 100)
            .await
            .unwrap();
        for fact in &facts {
            let pred = kg.relationship_type_name(fact.relationship_type_id).await;
            if pred.as_deref() != Some(predicate) {
                continue;
            }
            if let Some(object_id) = fact.object_id {
                if let Ok(Some(entity)) = kg.get_entity(object_id).await
                    && entity.name == object
                {
                    return true;
                }
            } else if fact.object_literal.as_deref() == Some(object) {
                return true;
            }
        }
    }
    false
}

/// Poll a condition until it holds or a 5-second deadline passes.
async fn wait_for<F, Fut>(mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !cond().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within 5 seconds"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The persisted fact for the user with the given predicate and object.
async fn find_fact(kg: &KnowledgeGraph, predicate: &str, object: &str) -> Option<Fact> {
    let search = kg.search_entities("Devansh", 10).await.unwrap();
    for result in &search {
        let facts = kg
            .get_facts_by_subject(result.entity.id, 100)
            .await
            .unwrap();
        for fact in &facts {
            let pred = kg.relationship_type_name(fact.relationship_type_id).await;
            if pred.as_deref() != Some(predicate) {
                continue;
            }
            if let Some(object_id) = fact.object_id {
                if let Ok(Some(entity)) = kg.get_entity(object_id).await
                    && entity.name == object
                {
                    return Some(fact.clone());
                }
            } else if fact.object_literal.as_deref() == Some(object) {
                return Some(fact.clone());
            }
        }
    }
    None
}

/// A two-year-old prose reminder addressed to the mailbox owner.
fn old_rent_email() -> Vec<u8> {
    b"From: landlord@example.com\r\n\
To: devansh@example.com\r\n\
Subject: Rent reminder\r\n\
Date: Tue, 20 Aug 2024 09:00:00 +0000\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Please pay rent by Friday.\r\n"
        .to_vec()
}

#[tokio::test]
async fn prose_prompt_carries_the_full_envelope() {
    // The LLM user turn must carry the message envelope — dates, sender,
    // recipients, bulk signals — plus the current date, so relative
    // phrases resolve against real timestamps (issue #398).
    let mock = llm_tool_response(r#"{"facts": []}"#);
    let raw = old_rent_email();
    let message = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let internal = chrono::FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2024, 8, 20, 9, 5, 0)
        .unwrap();
    let backend: Arc<dyn LlmBackend> = mock.clone();
    extract_prose_facts(
        &backend,
        Some("Devansh"),
        &message,
        "17:42",
        Some(internal),
        Some("devansh@example.com"),
        &taxonomy_names(),
    )
    .await
    .expect("extract");

    let calls = mock.system_chat_calls();
    assert_eq!(calls.len(), 1);
    let user = calls[0]
        .iter()
        .find(|m| m.role == "user")
        .expect("user turn");
    for needle in [
        "From: landlord@example.com",
        "To: devansh@example.com",
        "Sent: 2024-08-20T09:00:00",
        "Received: 2024-08-20T09:05:00",
        "List-Unsubscribe: absent",
        "Forwarded: no",
        "Misdirected: no",
        "Current date:",
        "Subject: Rent reminder",
        "Please pay rent by Friday.",
    ] {
        assert!(
            user.content.contains(needle),
            "prompt must include {needle:?}:\n{}",
            user.content
        );
    }
}

#[tokio::test]
async fn old_actionable_email_binds_past_valid_until() {
    // A two-year-old "pay rent" email must produce a historical fact, never
    // a current action item: the Rust binding anchors `valid_from` at the
    // sent date and expires the actionable window 30 days later — in the
    // past for old mail (issue #398 acceptance).
    let mock = llm_tool_response(
        r#"{"facts": [{
                "subject": "the user",
                "subject_type": "Person",
                "relationship_type": "has_appointment",
                "object": "pay rent",
                "object_is_entity": false,
                "requires_user_action": true
            }]}"#,
    );
    let raw = old_rent_email();
    let message = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let backend: Arc<dyn LlmBackend> = mock.clone();
    let facts = extract_prose_facts(
        &backend,
        Some("Devansh"),
        &message,
        "17:42",
        None,
        Some("devansh@example.com"),
        &taxonomy_names(),
    )
    .await
    .expect("extract")
    .facts;

    assert_eq!(facts.len(), 1);
    let fact = &facts[0];
    assert!(fact.requires_user_action);
    assert_eq!(
        fact.valid_from,
        Some(
            chrono::Utc
                .with_ymd_and_hms(2024, 8, 20, 9, 0, 0)
                .single()
                .unwrap()
        ),
        "valid_from anchors at the email's sent date"
    );
    let valid_until = fact.valid_until.expect("actionable fact has a window");
    assert!(
        valid_until < chrono::Utc::now(),
        "a two-year-old email's actionable window is in the past: {valid_until}"
    );
}

#[tokio::test]
async fn forwarded_email_facts_are_not_actionable() {
    // Forwarded mail conveys someone else's conversation: the model may
    // still emit real-world facts, but Rust downgrades them to information
    // (never `requires_user_action`, issue #398 acceptance).
    let mock = llm_tool_response(
        r#"{"facts": [{
                "subject": "the user",
                "subject_type": "Person",
                "relationship_type": "has_appointment",
                "object": "renew the lease",
                "object_is_entity": false,
                "temporal": {"valid_from": "2026-09-01T10:00:00Z"},
                "requires_user_action": true,
                "event_type": "Task"
            }]}"#,
    );
    let raw = String::from_utf8(old_rent_email())
        .unwrap()
        .replace("Subject: Rent reminder", "Subject: Fwd: Rent reminder")
        .into_bytes();
    let message = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let backend: Arc<dyn LlmBackend> = mock.clone();
    let facts = extract_prose_facts(
        &backend,
        Some("Devansh"),
        &message,
        "17:42",
        None,
        Some("devansh@example.com"),
        &taxonomy_names(),
    )
    .await
    .expect("extract")
    .facts;

    assert_eq!(facts.len(), 1);
    assert!(
        !facts[0].requires_user_action,
        "forwarded mail is never actionable"
    );
    assert_eq!(
        facts[0].valid_from,
        Some(
            chrono::Utc
                .with_ymd_and_hms(2026, 9, 1, 10, 0, 0)
                .single()
                .unwrap()
        ),
        "explicit timestamps survive the envelope binding"
    );
}

#[tokio::test]
async fn wrong_recipient_email_facts_are_not_actionable() {
    // Mail addressed to someone else (the owner is BCC'd) must not author
    // obligations for the owner (issue #398).
    let mock = llm_tool_response(
        r#"{"facts": [{
                "subject": "the user",
                "subject_type": "Person",
                "relationship_type": "has_appointment",
                "object": "file the return",
                "object_is_entity": false,
                "requires_user_action": true
            }]}"#,
    );
    let raw = String::from_utf8(old_rent_email())
        .unwrap()
        .replace("To: devansh@example.com", "To: other@example.com")
        .into_bytes();
    let message = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let backend: Arc<dyn LlmBackend> = mock.clone();
    let facts = extract_prose_facts(
        &backend,
        Some("Devansh"),
        &message,
        "17:42",
        None,
        Some("devansh@example.com"),
        &taxonomy_names(),
    )
    .await
    .expect("extract")
    .facts;

    assert_eq!(facts.len(), 1);
    assert!(
        !facts[0].requires_user_action,
        "misdirected mail is never actionable"
    );
}

#[tokio::test]
async fn old_actionable_email_lands_historical_fact_in_kb() {
    // End-to-end through the hook engine: the persisted fact carries the
    // envelope-derived past `valid_until`, so it can never surface as a
    // current action item (issue #398 acceptance).
    let mock = llm_tool_response(
        r#"{"facts": [{
                "subject": "the user",
                "subject_type": "Person",
                "relationship_type": "has_appointment",
                "object": "pay rent",
                "object_is_entity": false,
                "requires_user_action": true
            }]}"#,
    );
    let env = hook_env(Some(mock.clone()), None).await;
    stage(&env.connector, old_rent_email()).await;
    env.connector.extract().await.expect("extract");
    wait_for(|| async {
        find_fact(&env.kg, "has_appointment", "pay rent")
            .await
            .is_some()
    })
    .await;

    let fact = find_fact(&env.kg, "has_appointment", "pay rent")
        .await
        .expect("inserted fact");
    assert_eq!(
        fact.valid_from,
        Some(
            chrono::Utc
                .with_ymd_and_hms(2024, 8, 20, 9, 0, 0)
                .single()
                .unwrap()
        )
    );
    let valid_until = fact.valid_until.expect("actionable window persisted");
    assert!(
        valid_until < chrono::Utc::now(),
        "old mail's action window is in the past: {valid_until}"
    );
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
        vec!["imip:dentist-1@example.com".to_string()],
        "the CANCEL tombstone is still buffered for the supervisor"
    );
}

#[tokio::test]
async fn llm_layer_extracts_prose_when_no_deterministic_facts() {
    // A plain-prose appointment email yields nothing from layers 1-2, so
    // `extract()` enqueues a `connector_item.remember` hook and the hook
    // handler inserts the LLM's validated facts through the shared pipeline
    // (issue #386).
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
    let env = hook_env(Some(mock.clone()), None).await;
    stage(&env.connector, prose_email()).await;
    let facts = env.connector.extract().await.expect("extract");
    assert!(
        facts.is_empty(),
        "prose email yields no deterministic facts"
    );
    assert_eq!(
        env.engine
            .pending_depth_for("connector_item.remember")
            .await,
        1,
        "the prose email is enqueued as a hook instance"
    );
    wait_for(|| has_fact(&env.kg, "has_appointment", "Dentist check-up")).await;
    assert_eq!(mock.system_chat_calls().len(), 1);
}

#[tokio::test]
async fn llm_layer_drops_facts_with_non_canonical_predicates() {
    // The LLM schema allows any relationship_type string, so Rust validates
    // the emitted predicate against the canonical vocabulary before a fact is
    // built (issue #412): a non-canonical predicate must be warned and dropped
    // instead of auto-creating a `relationship_types` row on first sync.
    let mock = llm_tool_response(
        r#"{"facts": [{
                "subject": "the user",
                "subject_type": "Person",
                "relationship_type": "owes",
                "object": "the bank",
                "object_is_entity": false
            }]}"#,
    );
    let env = hook_env(Some(mock.clone()), None).await;
    stage(&env.connector, prose_email()).await;
    env.connector.extract().await.expect("extract");
    wait_for(|| async { mock.system_chat_calls().len() == 1 }).await;
    // The handler has run to completion once the queue drains, so the
    // absence of the fact proves the predicate was dropped (a fixed sleep
    // could pass before the insert attempt finished on a loaded runner).
    // `pending_depth()` alone can reach zero while the dispatched instance
    // is still inserting, so also wait for the running count to drain.
    wait_for(|| async {
        env.engine.pending_depth().await == 0 && env.engine.running_count().await == 0
    })
    .await;
    assert!(
        !has_fact(&env.kg, "owes", "the bank").await,
        "non-canonical predicate must be dropped"
    );
    // The unknown predicate must be visible, not silent: it is durably staged
    // and the connector row records the cumulative staged counter (#468/#508).
    let row = env
        .kg
        .get_connector_by_slug("gmail-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.facts_accepted, 0);
    assert_eq!(row.facts_dropped, 1);
    assert_eq!(row.facts_staged, 1);
}

#[tokio::test]
async fn llm_layer_spam_email_skips_call_and_yields_no_facts() {
    // An obvious bulk-marketing email is skipped by the Rust pre-filter
    // before any LLM call: no facts, no system-queue call.
    let mock = llm_tool_response(r#"{"facts": []}"#);
    let env = hook_env(Some(mock.clone()), None).await;
    let spam: Vec<u8> = b"From: promo@mailchimp.com\r\n\
Subject: 50% off everything\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Sale ends Sunday!\r\n"
        .to_vec();
    stage(&env.connector, spam).await;
    let facts = env.connector.extract().await.expect("extract");
    assert!(facts.is_empty());
    wait_for(|| async { env.engine.pending_depth().await == 0 }).await;
    assert!(mock.system_chat_calls().is_empty());
}

#[tokio::test]
async fn llm_failure_retries_through_the_hook_runner() {
    // A retryable LLM failure must not become a silent empty extraction:
    // the hook runner re-enqueues the instance with time-based backoff and
    // the next attempt succeeds (issue #386).
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
    let env = hook_env(Some(mock.clone()), None).await;
    stage(&env.connector, prose_email()).await;
    env.connector.extract().await.expect("extract");
    wait_for(|| has_fact(&env.kg, "has_appointment", "Dentist check-up")).await;
    assert_eq!(
        mock.system_chat_calls().len(),
        2,
        "one failed attempt, one successful retry"
    );
    assert_eq!(
        env.connector.prose_retry.lock().unwrap().terminal_count(),
        0
    );
}

#[tokio::test]
async fn llm_failure_is_bounded_and_terminates_after_max_attempts() {
    // Three failures exhaust the default budget: the hook handler records a
    // durable terminal failure and the message is never re-processed.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .build(),
    );
    let env = hook_env(Some(mock.clone()), None).await;
    stage(&env.connector, prose_email()).await;
    env.connector.extract().await.expect("extract");
    wait_for(|| async { mock.system_chat_calls().len() == 3 }).await;
    wait_for(|| async { env.connector.prose_retry.lock().unwrap().terminal_count() == 1 }).await;
    assert_eq!(
        env.engine.pending_depth().await,
        0,
        "terminal failure drops the instance"
    );
    // The terminal failure is recorded durably and stops future attempts.
    let durable = env
        .connector
        .durable_state()
        .expect("dirty ledger persists");
    let restored = crate::email::llm::retry::ProseRetryLedger::from_json(&durable);
    assert_eq!(restored.terminal_count(), 1);
    assert!(restored.is_terminal("17:42"));
}

#[tokio::test]
async fn configurable_max_attempts_fails_terminal_immediately() {
    // `llm_extraction_max_attempts: 1` bounds the retry budget to a single
    // attempt: the first failure is terminal, so no LLM call is ever
    // repeated.
    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .push_chat_error(mimir_core::llm::LlmError::QueueFull)
            .build(),
    );
    let env = hook_env(Some(mock.clone()), Some(1)).await;
    stage(&env.connector, prose_email()).await;
    env.connector.extract().await.expect("extract");
    wait_for(|| async { env.connector.prose_retry.lock().unwrap().terminal_count() == 1 }).await;
    assert_eq!(
        mock.system_chat_calls().len(),
        1,
        "no retry after a terminal failure"
    );
    assert_eq!(env.engine.pending_depth().await, 0);
}

#[tokio::test]
async fn malformed_message_records_durable_terminal_failure() {
    // A message the hook cannot parse can never be retried into a valid
    // RFC 822 message, so the handler must record a durable terminal ledger
    // entry — the connector health path reports failed extractions from the
    // ledger, and the item must not be re-staged on every cycle.
    let env = hook_env(None, None).await;
    let payload = crate::email::llm::EmailExtractionPayload {
        // An empty raw message has no headers, so `MessageParser` rejects it
        // (probe: `MessageParser::parse(&[])` is `None`).
        raw: Vec::new(),
        internal_date: None,
        mailbox_address: Some("devansh@example.com".to_string()),
        uid_validity: 17,
        uid: 43,
        raw_ref: "17:43".to_string(),
        user_identity: Some("Devansh".to_string()),
        instance_id: env.connector.instance_id,
        connector_type: env.connector.connector_type(),
        kg: Arc::clone(&env.kg),
        llm: Arc::new(MockLlmClient::builder().build()),
        ledger: Arc::clone(&env.connector.prose_retry),
        max_attempts: 3,
    };
    env.engine
        .trigger(Trigger::ConnectorItemStaged {
            item_id: "17:43".to_string(),
            payload: Arc::new(payload),
        })
        .await;
    wait_for(|| async { env.connector.prose_retry.lock().unwrap().terminal_count() == 1 }).await;
    wait_for(|| async { env.engine.pending_depth().await == 0 }).await;
    assert!(
        env.connector
            .prose_retry
            .lock()
            .unwrap()
            .is_terminal("17:43"),
        "malformed-message terminal failure must be durable in the ledger"
    );
    let durable = env
        .connector
        .durable_state()
        .expect("dirty ledger persists");
    let restored = crate::email::llm::retry::ProseRetryLedger::from_json(&durable);
    assert_eq!(restored.terminal_count(), 1);
}

#[tokio::test]
async fn restart_resume_re_stages_legacy_pending_retries_from_durable_state() {
    // Legacy migration (issue #386): a pending retry persisted by the
    // pre-hooks engine is drained at construction so its raw bytes re-stage
    // into the buffer and are re-enqueued as hooks on the next cycle.
    let legacy = serde_json::json!({
        "pending": {
            "17:42": {
                "uid_validity": 17,
                "uid": 42,
                "raw_b64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    prose_email(),
                ),
                "attempts": 1,
                "last_error": "queue full",
                "skip_cycles": 1
            }
        },
        "terminal": [],
        "tombstones": []
    });
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
    let env = hook_env(Some(mock.clone()), None).await;
    // Rebuild the connector seeded from the legacy durable state.
    let mut config = app_config();
    config["__instance_id"] = serde_json::json!(env.connector.instance_id);
    config["__durable_state"] = serde_json::Value::String(legacy.to_string());
    let restarted = EmailConnector::from_config_with_deps(
        config,
        EmailConnectorDeps {
            user_identity: Some("Devansh".to_string()),
            llm_backend: Some(mock.clone()),
            kg: Some(Arc::clone(&env.kg)),
            hook_engine: Some(Arc::clone(&env.engine)),
            ..Default::default()
        },
    )
    .expect("config");
    assert_eq!(
        restarted.buffer.lock().await.len(),
        1,
        "legacy pending retry must be re-staged from durable state"
    );
    // The next extraction cycle enqueues the re-staged message as a hook.
    restarted.extract().await.expect("extract");
    wait_for(|| has_fact(&env.kg, "has_appointment", "Dentist check-up")).await;
    assert_eq!(mock.system_chat_calls().len(), 1);
}

#[tokio::test]
async fn queue_full_re_stages_email_as_durable_overflow() {
    // Issue #442 review: a full `connector_item.remember` pending queue must
    // never advance the cursor past a staged email whose LLM extraction was
    // not enqueued. `extract()` records the message as a durable overflow
    // entry (raw bytes base64-encoded, bounded) instead of dropping it, and
    // the next extraction cycle re-stages and re-attempts it.
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
    // `max_pending = 0` makes every trigger report `QueueFull`.
    let env = hook_env_with_policy(Some(mock.clone()), None, Some(0)).await;
    stage(&env.connector, prose_email()).await;
    env.connector.extract().await.expect("extract");
    assert_eq!(
        env.engine.pending_depth().await,
        0,
        "a full queue must reject the enqueue"
    );
    assert!(
        !has_fact(&env.kg, "has_appointment", "Dentist check-up").await,
        "no hook instance ran while the queue was full"
    );
    let durable = env
        .connector
        .durable_state()
        .expect("queue-full overflow must be durable");
    let mut restored = crate::email::llm::retry::ProseRetryLedger::from_json(&durable);
    let drained = restored.drain_pending();
    assert_eq!(drained.len(), 1, "the rejected email must be recorded");
    assert_eq!(
        drained[0].raw().as_deref(),
        Some(prose_email().as_slice()),
        "the overflow record must carry the raw RFC 822 bytes"
    );

    // A still-full queue on the next cycle re-records the overflow instead
    // of losing the message; the email only leaves the ledger once the
    // enqueue succeeds.
    env.connector.extract().await.expect("extract");
    let durable_again = env
        .connector
        .durable_state()
        .expect("overflow must survive the retry cycle");
    let mut restored_again = crate::email::llm::retry::ProseRetryLedger::from_json(&durable_again);
    assert_eq!(restored_again.drain_pending().len(), 1);

    // Once the queue has room, a restarted connector seeded from the same
    // durable state re-stages the email and the hook extracts it (the IMAP
    // cursor has advanced past the message, so only the ledger can recover
    // it).
    let recovered = hook_env(Some(mock.clone()), None).await;
    let mut config = app_config();
    config["__instance_id"] = serde_json::json!(recovered.connector.instance_id);
    config["__durable_state"] = serde_json::Value::String(durable_again);
    let restarted = EmailConnector::from_config_with_deps(
        config,
        EmailConnectorDeps {
            user_identity: Some("Devansh".to_string()),
            llm_backend: Some(mock.clone()),
            kg: Some(Arc::clone(&recovered.kg)),
            hook_engine: Some(Arc::clone(&recovered.engine)),
            ..Default::default()
        },
    )
    .expect("config");
    assert_eq!(
        restarted.buffer.lock().await.len(),
        1,
        "the durable overflow must re-stage into the buffer"
    );
    restarted.extract().await.expect("extract");
    wait_for(|| has_fact(&recovered.kg, "has_appointment", "Dentist check-up")).await;
    assert_eq!(mock.system_chat_calls().len(), 1);
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
    mimir_core::llm::Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![mimir_core::llm::types::ToolCall {
            index: 0,
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: mimir_core::llm::types::FunctionCall {
                name: "extract_email_facts".to_string(),
                arguments: json.to_string(),
            },
        }]),
        tool_call_id: None,
    }
}

#[tokio::test]
async fn llm_layer_records_accepted_dropped_and_staged_fact_counters() {
    // The hook must persist cumulative accepted/dropped/staged fact counters
    // on the connector row (issues #468 and #508) so `mimir connector list` /
    // `status` shows the full extraction outcome instead of hiding data loss
    // behind `items`.
    let mock = llm_tool_response(
        r#"{"facts": [
                {"subject": "the user", "subject_type": "Person", "relationship_type": "has_appointment", "object": "Dentist check-up", "object_is_entity": true, "object_type": "Event"},
                {"subject": "the user", "subject_type": "Person", "relationship_type": "owes", "object": "the bank", "object_is_entity": false}
        ]}"#,
    );
    let env = hook_env(Some(mock.clone()), None).await;
    stage(&env.connector, prose_email()).await;
    env.connector.extract().await.expect("extract");
    wait_for(|| has_fact(&env.kg, "has_appointment", "Dentist check-up")).await;
    wait_for(|| async {
        env.kg
            .get_connector_by_slug("gmail-test")
            .await
            .unwrap()
            .unwrap()
            .facts_dropped
            == 1
    })
    .await;
    let row = env
        .kg
        .get_connector_by_slug("gmail-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.facts_accepted, 1);
    assert_eq!(row.facts_dropped, 1);
    assert_eq!(row.facts_staged, 1);
}
