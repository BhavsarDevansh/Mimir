mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use mimir_core::llm::types::{Message, Usage};
use mimir_core::llm::{LlmBackend, MockLlmClient};
use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::optimization::{OptimizationConfig, OptimizationRunner, PassName};

use common::TestGraph;

#[tokio::test]
async fn deterministic_dedup_merges_identical_fact_triples() {
    let graph = TestGraph::new().await;
    let person = graph.create_person("Devansh").await;
    let london = graph.create_place("London").await;

    let first = graph
        .create_fact(person, "lives_in", Some(london), SourceType::Connector)
        .await;
    let second = graph
        .create_fact(person, "lives_in", Some(london), SourceType::Import)
        .await;

    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph._dir.path().join("backups")),
        None,
    );

    let summary = runner.run_pass(PassName::Deduplication).await.unwrap();

    assert_eq!(summary.facts_merged, 1);

    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM facts WHERE subject_id = ? AND predicate_id = ? AND object_id = ? AND fact_status_id NOT IN (?, ?)",
    )
    .bind(person)
    .bind(first.predicate_id)
    .bind(london)
    .bind(FactStatus::Superseded as i16)
    .bind(FactStatus::Forgotten as i16)
    .fetch_one(graph.kg.pool())
    .await
    .unwrap();
    assert_eq!(active_count, 1);

    let source_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE fact_id = ?")
        .bind(first.id)
        .fetch_one(graph.kg.pool())
        .await
        .unwrap();
    assert_eq!(source_count, 2);

    let old_status: i16 = sqlx::query_scalar("SELECT fact_status_id FROM facts WHERE id = ?")
        .bind(second.id)
        .fetch_one(graph.kg.pool())
        .await
        .unwrap();
    assert_eq!(old_status, FactStatus::Superseded as i16);
}

#[tokio::test]
async fn semantic_dedup_queues_uncertain_llm_candidate() {
    let graph = TestGraph::new().await;
    let person = graph.create_person("Devansh").await;
    let rome = graph.create_place("Rome").await;

    graph
        .create_fact(person, "visited", Some(rome), SourceType::Connector)
        .await;
    graph
        .create_fact(person, "trip_to", Some(rome), SourceType::Import)
        .await;

    let llm: Arc<dyn LlmBackend> = Arc::new(
        MockLlmClient::builder()
            .push_chat(
                r#"{"candidates":[{"fact_a_id":1,"fact_b_id":2,"suggested_action":"merge","llm_confidence":0.8}]}"#,
                Usage::default(),
            )
            .build(),
    );
    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph._dir.path().join("backups")),
        Some(llm),
    );

    let summary = runner
        .run_pass(PassName::SemanticDeduplication)
        .await
        .unwrap();

    assert_eq!(summary.dedup_candidates_queued, 1);
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dedup_queue WHERE processed_at IS NULL")
            .fetch_one(graph.kg.pool())
            .await
            .unwrap();
    assert_eq!(queued, 1);
}

#[tokio::test]
async fn dormant_cleanup_forgets_old_disputed_non_user_fact() {
    let graph = TestGraph::new().await;
    let person = graph.create_person("Devansh").await;
    let old_place = graph.create_place("Old Place").await;
    let new_place = graph.create_place("New Place").await;

    let old_fact = graph
        .create_fact(person, "lives_in", Some(old_place), SourceType::Connector)
        .await;
    let counter_fact = graph
        .create_fact(person, "lives_in", Some(new_place), SourceType::Connector)
        .await;

    let old_updated_at = Utc::now() - Duration::days(31);
    sqlx::query("UPDATE facts SET fact_status_id = ?, confidence = ?, updated_at = ? WHERE id = ?")
        .bind(FactStatus::Disputed as i16)
        .bind(0.7_f32)
        .bind(old_updated_at)
        .bind(old_fact.id)
        .execute(graph.kg.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE facts SET fact_status_id = ?, confidence = ? WHERE id = ?")
        .bind(FactStatus::Disputed as i16)
        .bind(0.9_f32)
        .bind(counter_fact.id)
        .execute(graph.kg.pool())
        .await
        .unwrap();

    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph._dir.path().join("backups")),
        None,
    );

    let summary = runner.run_pass(PassName::DormantCleanup).await.unwrap();

    assert_eq!(summary.facts_forgotten, 1);
    let forgotten: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trash WHERE original_id = ?")
        .bind(old_fact.id)
        .fetch_one(graph.kg.pool())
        .await
        .unwrap();
    assert_eq!(forgotten, 1);
}

#[tokio::test]
async fn semantic_dedup_sends_strict_json_prompt_to_llm() {
    let graph = TestGraph::new().await;
    let person = graph.create_person("Devansh").await;
    let rome = graph.create_place("Rome").await;
    graph
        .create_fact(person, "visited", Some(rome), SourceType::Connector)
        .await;
    graph
        .create_fact(person, "trip_to", Some(rome), SourceType::Import)
        .await;

    let mock = Arc::new(
        MockLlmClient::builder()
            .push_chat(r#"{"candidates":[]}"#, Usage::default())
            .build(),
    );
    let llm: Arc<dyn LlmBackend> = mock.clone();
    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph._dir.path().join("backups")),
        Some(llm),
    );

    runner
        .run_pass(PassName::SemanticDeduplication)
        .await
        .unwrap();

    let calls = mock.chat_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].iter().any(|message| matches!(message, Message { role, content, .. } if role == "system" && content.contains("Return only JSON"))));
}
