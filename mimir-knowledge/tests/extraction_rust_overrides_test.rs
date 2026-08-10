//! Rust sensitivity gate overrides LLM false positives (Issue #142).

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

#[tokio::test]
async fn test_rust_overrides_llm_false_positive_small_flat() {
    // LLM says sensitive, but category 610 (Current Residence) and "small flat"
    // are not sensitive — Rust overrides to non-sensitive.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "based_in",
        "object": "small flat",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["610"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "I live in a small flat.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());
}

#[tokio::test]
async fn test_rust_overrides_llm_false_positive_chihuahuas() {
    // "I don't like chihuahuas" — LLM might flag as sensitive (relationship?),
    // but category 220 (Aversions & Dislikes) and "chihuahuas" are not sensitive.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "dislikes",
        "object": "chihuahuas",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["220"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "I don't like chihuahuas.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());
}

#[tokio::test]
async fn test_rust_confirms_llm_true_positive_allergy() {
    // LLM says sensitive + category 230 (Allergies) → Rust confirms → pending.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "allergy",
        "object": "peanuts",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["230"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "I am allergic to peanuts.")
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert_eq!(outcome.pending_confirmation.len(), 1);
}

#[tokio::test]
async fn test_rust_confirms_llm_true_positive_salary() {
    // LLM says sensitive + category 670 (Financial) → pending.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "salary",
        "object": "$100k",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["670"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "My salary is $100k.")
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert_eq!(outcome.pending_confirmation.len(), 1);
}

#[tokio::test]
async fn test_rust_confirms_llm_true_positive_diabetes() {
    // LLM says sensitive + category 320 (Current Conditions) → pending.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "health_condition",
        "object": "diabetes",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["320"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "I have diabetes.")
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert_eq!(outcome.pending_confirmation.len(), 1);
}

#[tokio::test]
async fn test_llm_non_sensitive_never_becomes_sensitive() {
    // LLM says non-sensitive, even though category 320 is sensitive — Rust
    // cannot widen.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "health_condition",
        "object": "diabetes",
        "object_is_entity": false,
        "is_sensitive": false,
        "categories": ["320"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "I have diabetes.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());
}

#[tokio::test]
async fn test_content_keyword_catches_miscategorised_sensitive_fact() {
    // LLM says sensitive but assigns a non-sensitive category — the content
    // keyword "allergic" catches it as a fallback.
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "dislikes",
        "object": "allergic to peanuts",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["220"]
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let outcome = tg
        .kg
        .extract_facts(&mock, "I am allergic to peanuts.")
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert_eq!(outcome.pending_confirmation.len(), 1);
}

// ---------------------------------------------------------------------------
// Events subsystem (#74): extraction creates event overlays
// ---------------------------------------------------------------------------

use mimir_knowledge::models::enums::{AutoCompletePolicy, EventType, RecurrenceType};

#[tokio::test]
async fn test_extraction_creates_event_for_future_dated_fact() {
    let tg = TestGraph::new().await;
    let future = chrono::Utc::now() + chrono::Duration::days(7);
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "is_in",
        "object": "Tokyo",
        "object_is_entity": false,
        "temporal": { "valid_from": future.to_rfc3339() },
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I am travelling to Tokyo next week.")
        .await
        .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let fact = &outcome.inserted[0];
    let event = tg.kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(
        event.auto_complete_policy_id,
        AutoCompletePolicy::AutoCompleteOnDate as i16
    );
    assert_eq!(event.event_type_id, EventType::Reminder as i16);
    assert!(!event.is_recurring());
}

#[tokio::test]
async fn test_extraction_creates_recurring_event_for_birthday() {
    let tg = TestGraph::new().await;
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "priya",
        "subject_type": "Person",
        "relationship_type": "is_in",
        "object": "birthday",
        "object_is_entity": false,
        "temporal": { "valid_from": "1995-06-15T00:00:00Z" },
        "recurrence": "yearly",
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "Priya's birthday is 15 June.")
        .await
        .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let fact = &outcome.inserted[0];
    let event = tg.kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(event.recurrence_type_id, RecurrenceType::Yearly as i16);
    assert_eq!(
        event.auto_complete_policy_id,
        AutoCompletePolicy::Recurring as i16
    );
    assert!(event.is_recurring());
}

#[tokio::test]
async fn test_extraction_creates_task_event_for_deadline() {
    let tg = TestGraph::new().await;
    let future = chrono::Utc::now() + chrono::Duration::days(2);
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "is_in",
        "object": "post letter",
        "object_is_entity": false,
        "temporal": { "valid_from": future.to_rfc3339() },
        "requires_user_action": true,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I have to post a letter by tomorrow 5pm.")
        .await
        .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let fact = &outcome.inserted[0];
    let event = tg.kg.get_event_by_fact(fact.id).await.unwrap().unwrap();
    assert_eq!(
        event.auto_complete_policy_id,
        AutoCompletePolicy::RequiresUserAction as i16
    );
    assert_eq!(event.event_type_id, EventType::Task as i16);
    assert!(event.requires_user_action);
}

#[tokio::test]
async fn test_extraction_skips_event_for_past_non_recurring_fact() {
    let tg = TestGraph::new().await;
    let past = chrono::Utc::now() - chrono::Duration::days(30);
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "favourite_colour",
        "object": "blue",
        "object_is_entity": false,
        "temporal": { "valid_from": past.to_rfc3339() },
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "Last month I liked blue.")
        .await
        .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    // A past, non-recurring, non-actionable fact must not get an event overlay.
    let event = tg
        .kg
        .get_event_by_fact(outcome.inserted[0].id)
        .await
        .unwrap();
    assert!(event.is_none());
}

#[tokio::test]
async fn test_sensitive_future_fact_gets_overlay_on_confirmation() {
    use mimir_knowledge::models::enums::{AutoCompletePolicy, EventType};

    let tg = TestGraph::new().await;
    let future = chrono::Utc::now() + chrono::Duration::days(10);

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "has_appointment",
        "object": "cardiology check-up",
        "object_is_entity": false,
        "temporal": { "valid_from": future.to_rfc3339() },
        "is_sensitive": true,
        "categories": ["230"]
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I have a cardiology check-up next week.")
        .await
        .unwrap();

    // Sensitive facts stay pending at extraction time and get no overlay yet.
    assert_eq!(outcome.pending_confirmation.len(), 1);
    let pending = &outcome.pending_confirmation[0];
    assert!(
        tg.kg
            .get_event_by_fact(pending.fact_id)
            .await
            .unwrap()
            .is_none(),
        "sensitive fact must not get an overlay before confirmation"
    );

    // Confirming creates the one-time event overlay.
    tg.kg.confirm_fact(pending.fact_id).await.unwrap();

    let event = tg
        .kg
        .get_event_by_fact(pending.fact_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.auto_complete_policy_id,
        AutoCompletePolicy::AutoCompleteOnDate as i16
    );
    assert_eq!(event.event_type_id, EventType::Reminder as i16);
    assert!(!event.is_recurring());
    assert!(!event.requires_user_action);

    // Re-confirming is a no-op for the overlay (idempotent) — the unique
    // constraint must not trip. confirm_fact refuses non-pending facts, so we
    // re-run the derive scan instead to exercise the idempotent insert path.
    let summary = tg.kg.run_events_scan(30).await.unwrap();
    assert_eq!(summary.derived, 0);
}
