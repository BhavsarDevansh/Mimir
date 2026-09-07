//! Tests for the deterministic context retrieval subsystem.

use std::sync::Arc;

use mimir_core::tools::ToolProgress;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
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

#[tokio::test]
async fn deterministic_retrieval_finds_conversation_for_natural_language_task() {
    // An ordinary research task must surface conversation evidence that
    // contains only some of the salient terms; the previous all-token AND
    // query rejected such messages unless every filler word matched too.
    let (kg, ctx, _dir) = setup().await;
    let session = ctx.create_session("test").await.unwrap();
    ctx.add_user_message(session, "Mary is allergic to shellfish")
        .await
        .unwrap();
    ctx.add_user_message(session, "the check-in time is 3pm")
        .await
        .unwrap();
    create_person(&kg, "Mary").await;

    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx);
    let result = agent
        .retrieve("Find Mary's food preferences and any allergies")
        .await
        .unwrap();

    assert!(
        result
            .conversation_snippets
            .iter()
            .any(|snippet| snippet.snippet.contains("allergic")),
        "expected the allergic message to surface, got {:?}",
        result.conversation_snippets
    );
    assert!(
        result
            .conversation_snippets
            .iter()
            .all(|snippet| !snippet.snippet.contains("check-in"))
    );
}

#[tokio::test]
async fn deterministic_retrieval_caps_plan_for_long_tasks() {
    // A token-spam task must stay within the planning caps instead of
    // fanning out into one search per token.
    let (kg, ctx, _dir) = setup().await;
    let task = (0..40)
        .map(|i| format!("entity{i}"))
        .collect::<Vec<_>>()
        .join(" ");

    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx);
    let result = agent.retrieve(&task).await.unwrap();

    // 16 capped token searches + 1 conversation search; the empty graph
    // yields no follow-up candidates.
    assert_eq!(result.steps_executed, 17);
}

#[tokio::test]
async fn deterministic_retrieval_strips_uppercase_possessives() {
    let (kg, ctx, _dir) = setup().await;
    create_person(&kg, "James").await;

    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx);
    let result = agent.retrieve("JAMES'S appointment").await.unwrap();

    // Possessives are stripped case-insensitively: one search for the name,
    // one for "appointment", one conversation search, and one candidate
    // (James) queried twice. No junk `S` token may be planned.
    assert_eq!(result.steps_executed, 5);
    assert!(result.entities.iter().all(|entity| entity.name != "S"));
}

#[tokio::test]
async fn deterministic_retrieval_paginates_kg_query_facts() {
    // Entities with more than one page of matching facts must have every
    // fact retrieved (up to the documented cap), none silently truncated.
    let (kg, ctx, _dir) = setup().await;
    let mary = kg
        .create_entity(
            "Mary",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap()
        .id;
    for i in 0..55 {
        let new_fact = NewFact {
            subject_id: mary,
            relationship_type: "skill".to_string(),
            object_id: None,
            object_literal: Some(format!("topic{i}")),
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: Some(1.0),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        };
        kg.insert_fact(new_fact).await.unwrap();
    }

    let agent = RetrievalAgent::new(Arc::clone(&kg), ctx);
    let result = agent.retrieve("Mary").await.unwrap();

    let mary_entity = result.entities.iter().find(|e| e.name == "Mary").unwrap();
    assert_eq!(mary_entity.facts.len(), 55);
}
