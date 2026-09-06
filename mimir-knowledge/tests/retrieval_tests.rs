//! Tests for the deterministic context retrieval subsystem.

use std::sync::Arc;

use mimir_core::tools::ToolProgress;
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

async fn create_person(kg: &KnowledgeGraph, name: &str) {
    kg.create_entity(
        name,
        mimir_knowledge::models::entity::EntityType::Person,
        &[],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn deterministic_retrieval_queries_each_candidate_once() {
    let (kg, ctx, _dir) = setup().await;
    create_person(&kg, "Mary").await;
    create_person(&kg, "Bob").await;

    let agent = RetrievalAgent::new(Arc::clone(&kg), Arc::clone(&ctx));
    let result = agent.retrieve("Mary Bob").await.unwrap();

    let mary = result.entities.iter().find(|e| e.name == "Mary").unwrap();
    let bob = result.entities.iter().find(|e| e.name == "Bob").unwrap();
    assert_eq!(mary.facts.len(), 0);
    assert_eq!(bob.facts.len(), 0);
    assert_eq!(result.steps_executed, 7);
    assert_eq!(result.finish_reason, Some("completed".to_string()));
}

#[tokio::test]
async fn deterministic_retrieval_searches_conversation_without_llm() {
    let (kg, ctx, _dir) = setup().await;
    let session = ctx.create_session("test").await.unwrap();
    ctx.add_user_message(session, "Mary likes shellfish")
        .await
        .unwrap();
    create_person(&kg, "Mary").await;

    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx);
    let result = agent.retrieve("Mary shellfish").await.unwrap();

    assert!(
        result
            .conversation_snippets
            .iter()
            .any(|snippet| snippet.snippet.contains("likes")
                && snippet.snippet.contains("shellfish"))
    );
}

#[tokio::test]
async fn deterministic_retrieval_emits_progress_for_each_step() {
    let (kg, ctx, _dir) = setup().await;
    create_person(&kg, "Mary").await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ToolProgress>(16);
    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx).with_progress(tx);
    let result = agent.retrieve("Mary").await.unwrap();
    drop(agent);

    assert_eq!(result.steps_executed, 4);
    let mut starts = 0;
    while let Some(event) = rx.recv().await {
        match event {
            ToolProgress::Started { .. } => starts += 1,
            ToolProgress::Finished { .. } => {}
        }
    }
    assert_eq!(starts, 4);
}

#[tokio::test]
async fn deterministic_retrieval_reports_tool_start_before_finish() {
    let (kg, ctx, _dir) = setup().await;
    create_person(&kg, "Mary").await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ToolProgress>(8);
    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx).with_progress(tx);
    agent.retrieve("Mary").await.unwrap();
    drop(agent);

    let started_at = rx
        .recv()
        .await
        .map(|event| matches!(event, ToolProgress::Started { .. }));
    assert_eq!(started_at, Some(true));
}
