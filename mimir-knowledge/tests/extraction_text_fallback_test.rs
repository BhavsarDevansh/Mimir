//! Fallback extraction tests: JSON text (no tool call) and unknown predicates.

use std::sync::Arc;

use mimir_core::llm::MockLlmClient;
use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};

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

fn build_mock_with_text_output(content: String) -> Arc<dyn mimir_core::llm::backend::LlmBackend> {
    let msg = Message {
        role: "assistant".to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
    };

    Arc::new(
        MockLlmClient::builder()
            .push_chat_message(msg, Usage::default())
            .build(),
    )
}

#[tokio::test]
async fn test_text_fallback_with_wrapper() {
    let tg = TestGraph::new().await;

    let text = serde_json::json!({
        "facts": [{
            "classification": "Explicit",
            "subject": "devansh",
            "subject_type": "Person",
            "relationship_type": "favourite_colour",
            "object": "green",
            "object_is_entity": false,
            "is_sensitive": false
        }]
    })
    .to_string();

    let mock = build_mock_with_text_output(text);
    let result = tg
        .kg
        .extract_facts(&mock, "My favourite colour is green.")
        .await
        .unwrap();

    assert_eq!(result.inserted.len(), 1);
    assert_eq!(result.inserted[0].object_literal.as_deref(), Some("green"));
}

#[tokio::test]
async fn test_text_fallback_with_markdown_block() {
    let tg = TestGraph::new().await;

    let text = format!(
        "```json\n{}\n```",
        serde_json::json!({
            "facts": [{
                "classification": "Explicit",
                "subject": "devansh",
                "subject_type": "Person",
                "relationship_type": "favourite_colour",
                "object": "yellow",
                "object_is_entity": false,
                "is_sensitive": false
            }]
        })
    );

    let mock = build_mock_with_text_output(text);
    let result = tg
        .kg
        .extract_facts(&mock, "My favourite colour is yellow.")
        .await
        .unwrap();

    assert_eq!(result.inserted.len(), 1);
    assert_eq!(result.inserted[0].object_literal.as_deref(), Some("yellow"));
}

#[tokio::test]
async fn test_text_fallback_with_bare_array() {
    let tg = TestGraph::new().await;

    let text = serde_json::json!([
        {
            "classification": "Explicit",
            "subject": "devansh",
            "subject_type": "Person",
            "relationship_type": "favourite_colour",
            "object": "red",
            "object_is_entity": false,
            "is_sensitive": false
        }
    ])
    .to_string();

    let mock = build_mock_with_text_output(text);
    let result = tg
        .kg
        .extract_facts(&mock, "My favourite colour is red.")
        .await
        .unwrap();

    assert_eq!(result.inserted.len(), 1);
    assert_eq!(result.inserted[0].object_literal.as_deref(), Some("red"));
}

// ---------------------------------------------------------------------------
// Issue #136: alias + hierarchy is the single source of truth; the deprecated
// hardcoded `normalize_predicate` map is removed. These tests lock in the new
// behaviour: unknown predicates are auto-created as canonical types via
// `ensure_relationship_type`, and predicate resolution happens before entity
// validation so a rejected fact still registers its predicate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_predicate_auto_created_no_split() {
    let tg = TestGraph::new().await;

    // "wibbles_at" is not a seeded alias or canonical type.
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "wibbles_at",
        "object": "Guitar",
        "object_is_entity": false,
        "categories": [],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(&mock, "devansh wibbles_at Guitar.")
        .await
        .unwrap();

    // One fact inserted — no list-splitting for an unknown predicate.
    assert_eq!(result.inserted.len(), 1);
    assert!(result.errors.is_empty());

    // The predicate was registered as a canonical relationship type.
    let id = tg.kg.get_relationship_type_id("wibbles_at").await.unwrap();
    assert!(id.is_some(), "unknown predicate should be auto-created");
    let fact = &result.inserted[0];
    assert_eq!(
        tg.kg
            .relationship_type_name(fact.relationship_type_id)
            .await
            .as_deref(),
        Some("wibbles_at")
    );
    assert_eq!(fact.object_literal.as_deref(), Some("Guitar"));
}

#[tokio::test]
async fn test_unknown_predicate_registered_even_when_fact_rejected() {
    let tg = TestGraph::new().await;

    // Valid predicate but an invalid subject_type: the fact must be rejected,
    // yet the predicate is resolved (and auto-created) before entity validation.
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "BogusType",
        "relationship_type": "frobnicates",
        "object": "Widget",
        "object_is_entity": false,
        "categories": [],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(&mock, "devansh frobnicates Widget.")
        .await
        .unwrap();

    // The fact itself is rejected, but the batch tolerated the error.
    assert!(result.inserted.is_empty());
    assert_eq!(result.errors.len(), 1);

    // The predicate was still registered via the alias pipeline.
    let id = tg.kg.get_relationship_type_id("frobnicates").await.unwrap();
    assert!(
        id.is_some(),
        "predicate should be registered even when its fact is rejected"
    );
}
