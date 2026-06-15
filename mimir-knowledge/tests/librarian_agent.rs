//! Integration tests for the Librarian Agent background extraction pipeline.

use std::sync::Arc;

use mimir_core::agents::Agent;
use mimir_core::conversation::ConversationTurn;
use mimir_core::identity::UserIdentity;
use mimir_core::llm::MockLlmClient;
use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};

use mimir_knowledge::librarian::{LibrarianAgent, LibrarianContext, LibrarianGoal};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::FactStatus;

fn make_remember_tool_output(facts: Vec<serde_json::Value>) -> String {
    serde_json::json!({ "facts": facts }).to_string()
}

fn build_mock(
    tool_args: String) -> (Arc<MockLlmClient>, Arc<dyn mimir_core::llm::backend::LlmBackend>) {
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

    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(msg, Usage::default())
            .build(),
    );
    let backend: Arc<dyn mimir_core::llm::backend::LlmBackend> = mock.clone() as Arc<dyn mimir_core::llm::backend::LlmBackend>;
    (mock, backend)
}

async fn setup_kg() -> (Arc<mimir_knowledge::KnowledgeGraph>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = mimir_knowledge::KnowledgeGraph::init(&dir.path().join("knowledge.db"),
    )
    .await
    .unwrap();
    (Arc::new(kg), dir)
}

#[tokio::test]
async fn librarian_extracts_fact_from_conversation_turn() {
    let (kg, _dir) = setup_kg().await;
    let user_id = kg
        .create_entity("devansh", EntityType::Person, &[])
        .await
        .unwrap()
        .id;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "favourite_colour",
        "object": "green",
        "object_is_entity": false,
        "is_sensitive": false
    })]);
    let (_mock, backend) = build_mock(tool_args);

    let turn = ConversationTurn::new(
        1,
        "My favourite colour is green.",
        "Noted! I will remember that.",
    );
    let goal = LibrarianGoal::new(user_id, "chat-turn-extraction", turn);
    let identity = UserIdentity::new("devansh", user_id);
    let ctx = LibrarianContext::new(
        Arc::clone(&kg),
        Arc::clone(&backend),
        identity,
        Some("User likes colours.".to_string()),
    );

    let agent = LibrarianAgent::new();
    agent.run(goal, Arc::new(ctx)).await.unwrap();

    let facts = kg.get_facts_by_subject(user_id, 10).await.unwrap();
    assert_eq!(facts.len(), 1);
    let fact = &facts[0];
    assert_eq!(fact.confidence, 1.0);
    assert_eq!(fact.status(), Some(FactStatus::Active));

    let audit = kg.get_audit_log(fact.id).await.unwrap();
    assert!(!audit.is_empty());
    assert_eq!(audit[0].change_type_id, mimir_knowledge::models::audit_log::ChangeType::Created as i16);
}

#[tokio::test]
async fn librarian_prompt_includes_transcript_and_memory() {
    let (kg, _dir) = setup_kg().await;
    let user_id = kg
        .create_entity("devansh", EntityType::Person, &[])
        .await
        .unwrap()
        .id;

    let tool_args = make_remember_tool_output(vec![]);
    let (mock, backend) = build_mock(tool_args);

    let turn = ConversationTurn::new(
        1,
        "I just moved to Berlin.",
        "Berlin is a great city.",
    );
    let goal = LibrarianGoal::new(user_id, "chat-turn-extraction", turn.clone());
    let identity = UserIdentity::new("devansh", user_id);
    let ctx = LibrarianContext::new(
        Arc::clone(&kg),
        Arc::clone(&backend),
        identity,
        Some("Previously lived in London.".to_string()),
    );

    let agent = LibrarianAgent::new();
    let _ = agent.run(goal, Arc::new(ctx)).await;

    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 1);
    let transcript = format!(
        "User: {}\nAssistant: {}",
        turn.user_message, turn.assistant_response
    );
    assert_eq!(calls[0].len(), 2);
    assert!(calls[0][0].content.contains("User identity:"));
    assert!(calls[0][1].content.contains(&transcript));
    assert!(calls[0][0].content.contains("Previously lived in London."));
}
