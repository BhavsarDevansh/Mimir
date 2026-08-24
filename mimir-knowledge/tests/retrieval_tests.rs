//! Tests for the agentic retrieval subsystem.

use mimir_core::tools::Tool;
use std::sync::Arc;

use mimir_core::llm::mock::MockLlmClient;
use mimir_core::llm::types::{Message, ToolCall, Usage};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::retrieval::RetrievalAgent;

async fn setup() -> (
    Arc<KnowledgeGraph>,
    Arc<mimir_core::context::ContextManager>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let kg_path = dir.path().join("knowledge.db");
    let ctx_path = dir.path().join("context.db");

    let kg = Arc::new(KnowledgeGraph::init(&kg_path).await.unwrap());
    let ctx = Arc::new(
        mimir_core::context::ContextManager::new(&ctx_path)
            .await
            .unwrap(),
    );

    (kg, ctx, dir)
}

fn make_tool_call(name: &str, args: serde_json::Value) -> Message {
    Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCall {
            index: 0,
            id: format!("call_{}", name),
            call_type: "function".to_string(),
            function: mimir_core::llm::types::FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }]),
        tool_call_id: None,
    }
}

// ------------------------------------------------------------------
// Test 1: Single-round retrieval (kg_query + finish_retrieval)
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieval_single_round() {
    let (kg, ctx, _dir) = setup().await;

    // Seed a test entity.
    let _entity = kg
        .create_entity(
            "Mary",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    let mock = MockLlmClient::builder()
        // Round 0: call kg_query for Mary
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Mary"})),
            Usage::default(),
        )
        // Round 1: finish retrieval
        .push_chat_message(
            make_tool_call("finish_retrieval", serde_json::json!({"reason": "done"})),
            Usage::default(),
        )
        .build();

    let agent = RetrievalAgent::new(Arc::new(mock), kg, ctx);
    let result = agent.retrieve("Find Mary's preferences").await.unwrap();

    // Should have found Mary as an entity (even if no facts were returned).
    assert!(result.entities.iter().any(|e| e.name == "Mary"));
    assert_eq!(result.rounds_used, 2);
}

// ------------------------------------------------------------------
// Test 2: Multi-step retrieval
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieval_multi_step() {
    let (kg, ctx, _dir) = setup().await;

    let mock = MockLlmClient::builder()
        // Round 0: kg_query Mary
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Mary"})),
            Usage::default(),
        )
        // Round 1: search_conversation_history + kg_related
        .push_chat_message(
            make_tool_call(
                "search_conversation_history",
                serde_json::json!({"query": "Mary dinner", "limit": 5}),
            ),
            Usage::default(),
        )
        // Round 2: finish
        .push_chat_message(
            make_tool_call("finish_retrieval", serde_json::json!({"reason": "done"})),
            Usage::default(),
        )
        .build();

    let agent = RetrievalAgent::new(Arc::new(mock), kg, ctx);
    let result = agent
        .retrieve("What does Mary like for dinner?")
        .await
        .unwrap();

    assert_eq!(result.rounds_used, 3);
}

// ------------------------------------------------------------------
// Test 3: Max rounds circuit breaker
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieval_max_rounds_circuit_breaker() {
    let (kg, ctx, _dir) = setup().await;

    // Mock LLM never calls finish_retrieval — always kg_query.
    let mut builder = MockLlmClient::builder();
    for _ in 0..RetrievalAgent::MAX_ROUNDS {
        builder = builder.push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Unknown"})),
            Usage::default(),
        );
    }
    let mock = builder.build();

    let agent = RetrievalAgent::new(Arc::new(mock), kg, ctx);
    let result = agent.retrieve("Find something").await.unwrap();

    assert_eq!(result.rounds_used, RetrievalAgent::MAX_ROUNDS);
}

// ------------------------------------------------------------------
// Test 4: Dedup on duplicate queries
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieval_dedup_duplicate_queries() {
    let (kg, ctx, _dir) = setup().await;

    // Create an entity so kg_query returns something real.
    let _entity = kg
        .create_entity(
            "Bob",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    let mock = MockLlmClient::builder()
        // Round 0: kg_query Bob
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Bob"})),
            Usage::default(),
        )
        // Round 1: kg_query Bob again
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Bob"})),
            Usage::default(),
        )
        // Round 2: finish
        .push_chat_message(
            make_tool_call("finish_retrieval", serde_json::json!({})),
            Usage::default(),
        )
        .build();

    let agent = RetrievalAgent::new(Arc::new(mock), kg, ctx);
    let result = agent.retrieve("Find Bob's info").await.unwrap();

    // Should only have one Bob entity.
    assert_eq!(
        result.entities.iter().filter(|e| e.name == "Bob").count(),
        1
    );
    assert_eq!(result.rounds_used, 3);
}

// ------------------------------------------------------------------
// Test 5: Error resilience — tool failure doesn't crash agent
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieval_error_resilience() {
    let (kg, ctx, _dir) = setup().await;

    let mock = MockLlmClient::builder()
        // Round 0: call a tool that will fail (invalid args)
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": ""})),
            Usage::default(),
        )
        // Round 1: finish anyway
        .push_chat_message(
            make_tool_call("finish_retrieval", serde_json::json!({})),
            Usage::default(),
        )
        .build();

    let agent = RetrievalAgent::new(Arc::new(mock), kg, ctx);
    let result = agent.retrieve("Find something").await.unwrap();

    // Should complete despite the tool error.
    assert_eq!(result.rounds_used, 2);
}

// ------------------------------------------------------------------
// Test 6: RetrieveContextTool integration
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieve_context_tool_integration() {
    use mimir_knowledge::tools::RetrieveContextTool;

    let (kg, ctx, _dir) = setup().await;

    let mock = MockLlmClient::builder()
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Alice"})),
            Usage::default(),
        )
        .push_chat_message(
            make_tool_call("finish_retrieval", serde_json::json!({})),
            Usage::default(),
        )
        .build();

    let tool = RetrieveContextTool::new(kg, ctx, Arc::new(mock));
    let output = tool
        .execute(serde_json::json!({"task": "Find Alice"}))
        .await
        .unwrap();

    assert!(output.result.is_some());
    assert!(output.stdout.as_ref().unwrap().contains("Retrieved"));
}

// ------------------------------------------------------------------
// Test 7: Progress events for sub-tool calls
// ------------------------------------------------------------------

#[tokio::test]
async fn retrieval_emits_progress_for_sub_tool_calls() {
    use mimir_core::tools::ToolProgress;

    let (kg, ctx, _dir) = setup().await;

    let mock = MockLlmClient::builder()
        // Round 0: kg_query
        .push_chat_message(
            make_tool_call("kg_query", serde_json::json!({"entity_name": "Mary"})),
            Usage::default(),
        )
        // Round 1: finish
        .push_chat_message(
            make_tool_call("finish_retrieval", serde_json::json!({})),
            Usage::default(),
        )
        .build();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ToolProgress>(16);
    let agent = RetrievalAgent::new(Arc::new(mock), kg, ctx).with_progress(tx);
    let result = agent.retrieve("Find Mary's preferences").await.unwrap();
    assert_eq!(result.rounds_used, 2);

    // Round 0 emits Started then Finished for kg_query.
    match rx.recv().await.unwrap() {
        ToolProgress::Started { name, display_name } => {
            assert_eq!(name, "kg_query");
            assert_eq!(display_name, "Kg Query");
        }
        other => panic!("expected Started, got {other:?}"),
    }
    match rx.recv().await.unwrap() {
        ToolProgress::Finished { name, result, .. } => {
            assert_eq!(name, "kg_query");
            assert!(!result.is_empty(), "finished result should be non-empty");
        }
        other => panic!("expected Finished, got {other:?}"),
    }

    // No further events: finish_retrieval is not reported.
    assert!(rx.try_recv().is_err(), "unexpected extra progress event");
}
