//! Integration tests for the fact extraction pipeline (Issue #55).

use std::sync::Arc;

use mimir_core::llm::MockLlmClient;
use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};

use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::source::SourceType;

mod common;
use common::TestGraph;

fn make_remember_tool_output(facts: Vec<serde_json::Value>) -> String {
    serde_json::json!({ "facts": facts }).to_string()
}

fn build_mock_with_tool_output(tool_args: String) -> Arc<dyn mimir_core::llm::backend::LlmBackend> {
    let msg = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCall {
            index: 0,
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: tool_args,
            },
        }]),
        tool_call_id: None,
    };

    Arc::new(
        MockLlmClient::builder()
            .push_chat_message(msg, Usage::default())
            .build(),
    )
}

// ---------------------------------------------------------------------------
// Test 1: Explicit extraction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_explicit_extraction() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "favourite_colour",
        "object": "blue",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "My favourite colour is blue.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());
    assert!(outcome.errors.is_empty());

    let fact = &outcome.inserted[0];
    assert_eq!(fact.confidence, 1.0);
    assert_eq!(fact.status(), Some(FactStatus::Active));
    assert!(!fact.pending_confirmation);

    let sources = tg.kg.get_sources_for_fact(fact.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::UserEdit as i16);
}

// ---------------------------------------------------------------------------
// Test 2: Casual extraction (lower confidence, no overwrite)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_casual_extraction() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    // Pre-insert an explicit favourite_colour.
    tg.create_fact(devansh, "favourite_colour", None, SourceType::UserEdit)
        .await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Casual",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "favourite_colour",
        "object": "green",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "Green is a nice colour.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let fact = &outcome.inserted[0];
    assert_eq!(fact.confidence, 0.30);
    assert_eq!(fact.status(), Some(FactStatus::Disputed));

    // The old explicit fact should still exist and be Active.
    let old_facts = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let explicit = old_facts.iter().find(|f| f.confidence == 1.0).unwrap();
    assert_eq!(explicit.status(), Some(FactStatus::Disputed));
}

// ---------------------------------------------------------------------------
// Test 3: Entity resolution (existing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_resolution_existing() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "likes",
        "object": "coding",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "devansh likes coding.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(outcome.inserted[0].subject_id, devansh);
}

// ---------------------------------------------------------------------------
// Test 4: Entity creation (new)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_creation_new() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "alice",
        "subject_type": "Person",
        "predicate": "works_as",
        "object": "engineer",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "Alice works as an engineer.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);

    let alice = tg
        .kg
        .get_entity(outcome.inserted[0].subject_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice.name, "alice");
    assert_eq!(alice.entity_type_id, EntityType::Person as i16);
}

// ---------------------------------------------------------------------------
// Test 5: Temporal correction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_temporal_correction() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    // Pre-insert an open-ended fact.
    tg.create_fact_with_temporal(devansh, "lives_in", None, None, None, SourceType::UserEdit)
        .await;

    let now = chrono::Utc::now();
    let scope = now.to_rfc3339();

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Correction",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "lives_in",
        "object": "Manchester",
        "object_is_entity": false,
        "correction_scope": scope,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "As of now I live in Manchester.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let new_fact = &outcome.inserted[0];
    assert_eq!(new_fact.status(), Some(FactStatus::Active));

    // Old fact should have been closed.
    let facts = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let old = facts.iter().find(|f| f.id != new_fact.id).unwrap();
    assert!(old.valid_until.is_some());
    assert_eq!(old.status(), Some(FactStatus::Superseded));
}

// ---------------------------------------------------------------------------
// Test 6: Retrospective correction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retrospective_correction() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    // Pre-insert an active fact.
    let old = tg
        .create_fact(devansh, "favourite_colour", None, SourceType::UserEdit)
        .await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Correction",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "favourite_colour",
        "object": "green",
        "object_is_entity": false,
        "correction_scope": "always",
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(
            &mock,
            "My favourite colour has always been green, not blue.",
        )
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let new_fact = &outcome.inserted[0];
    assert_eq!(new_fact.status(), Some(FactStatus::Active));
    assert_eq!(new_fact.confidence, 1.0);

    // Old fact should be in trash.
    let old_fact = tg.kg.get_fact(old.id).await.unwrap();
    assert!(old_fact.is_none());
}

// ---------------------------------------------------------------------------
// Test 7: Sensitive fact confirmation flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sensitive_fact_confirmation() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "allergy",
        "object": "peanuts",
        "object_is_entity": false,
        "is_sensitive": true
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I am allergic to peanuts.")
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert_eq!(outcome.pending_confirmation.len(), 1);

    let pending = &outcome.pending_confirmation[0];
    assert_eq!(pending.predicate, "allergy");
    assert_eq!(pending.object_display, "peanuts");

    // Verify DB state.
    let fact = tg.kg.get_fact(pending.fact_id).await.unwrap().unwrap();
    assert_eq!(fact.status(), Some(FactStatus::Disputed));
    assert!(fact.pending_confirmation);

    // Confirm the fact.
    let confirmed = tg.kg.confirm_fact(pending.fact_id).await.unwrap();
    assert_eq!(confirmed.status(), Some(FactStatus::Active));
    assert_eq!(confirmed.confidence, 1.0);
    assert!(!confirmed.pending_confirmation);

    // Verify cache is clear.
    assert!(
        !tg.kg
            .pending_confirmations()
            .read()
            .await
            .contains(&pending.fact_id)
    );
}

// ---------------------------------------------------------------------------
// Test 8: Multiple facts in one message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_facts() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![
        serde_json::json!({
            "classification": "Explicit",
            "subject": "devansh",
            "subject_type": "Person",
            "predicate": "favourite_colour",
            "object": "blue",
            "object_is_entity": false,
            "is_sensitive": false
        }),
        serde_json::json!({
            "classification": "Casual",
            "subject": "devansh",
            "subject_type": "Person",
            "predicate": "likes",
            "object": "pizza",
            "object_is_entity": false,
            "is_sensitive": false
        }),
    ]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "My favourite colour is blue. I also like pizza.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 2);
    assert!(outcome.pending_confirmation.is_empty());
    assert!(outcome.errors.is_empty());
}

// ---------------------------------------------------------------------------
// Test 9: Invalid LLM output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_invalid_llm_output() {
    let tg = TestGraph::new().await;

    let msg = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCall {
            index: 0,
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: "not json".to_string(),
            },
        }]),
        tool_call_id: None,
    };

    let mock: Arc<dyn mimir_core::llm::backend::LlmBackend> = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(msg, Usage::default())
            .build(),
    );

    let result = tg.kg.extract_facts(&mock, "This is a test.").await;

    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Test 10: Empty extraction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_empty_extraction() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg.kg.extract_facts(&mock, "Hello!").await.unwrap();

    assert!(outcome.inserted.is_empty());
    assert!(outcome.pending_confirmation.is_empty());
    assert!(outcome.errors.is_empty());
}

// ---------------------------------------------------------------------------
// Test 11: Reject sensitive fact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reject_sensitive_fact() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "predicate": "allergy",
        "object": "shellfish",
        "object_is_entity": false,
        "is_sensitive": true
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I am allergic to shellfish.")
        .await
        .unwrap();

    let pending = &outcome.pending_confirmation[0];

    // Reject the fact.
    tg.kg.reject_fact(pending.fact_id).await.unwrap();

    // Verify deletion.
    assert!(tg.kg.get_fact(pending.fact_id).await.unwrap().is_none());
    assert!(
        !tg.kg
            .pending_confirmations()
            .read()
            .await
            .contains(&pending.fact_id)
    );
}
