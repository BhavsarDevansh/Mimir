mod common;

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};
use mimir_core::llm::{LlmBackend, MockLlmClient};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::clock::MockClock;
use mimir_knowledge::extract::{
    Classification, ExtractedFact, RememberOutput, process_remember_output,
};
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
        "SELECT COUNT(*) FROM facts WHERE subject_id = ? AND relationship_type_id = ? AND object_id = ? AND fact_status_id NOT IN (?, ?)",
    )
    .bind(person)
    .bind(first.relationship_type_id)
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

    let fact_a = graph
        .create_fact(person, "visited", Some(rome), SourceType::Connector)
        .await;
    let fact_b = graph
        .create_fact(person, "trip_to", Some(rome), SourceType::Import)
        .await;

    let args = format!(
        r#"{{"candidates":[{{"fact_a_id":{},"fact_b_id":{},"suggested_action":"merge","llm_confidence":0.8}}]}}"#,
        fact_a.id, fact_b.id
    );

    let llm: Arc<dyn LlmBackend> = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_calls: Some(vec![ToolCall {
                        index: 0,
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "evaluate_dedup_candidates".to_string(),
                            arguments: args,
                        },
                    }]),
                    tool_call_id: None,
                },
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
            .push_chat_message(
                Message {
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_calls: Some(vec![ToolCall {
                        index: 0,
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "evaluate_dedup_candidates".to_string(),
                            arguments: r#"{"candidates":[]}"#.to_string(),
                        },
                    }]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
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
    let tools = mock.chat_tools();
    assert_eq!(tools.len(), 1);
    let tool = tools[0].as_ref().unwrap().first().unwrap();
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["function"]["name"], "evaluate_dedup_candidates");
}

/// Build a fresh `KnowledgeGraph` driven by a [`MockClock`] so the pending
/// cleanup pass can fast-forward past a retention window.
async fn pending_kg_with_clock(
    start: DateTime<Utc>,
) -> (KnowledgeGraph, Arc<MockClock>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(MockClock::new(start));
    let kg = KnowledgeGraph::init_with_clock(&dir.path().join("pending_opt.db"), clock.clone())
        .await
        .unwrap();
    (kg, clock, dir)
}

/// Insert a pending sensitive fact at the clock's current time and return its id.
async fn insert_pending_fact(kg: &KnowledgeGraph, object: &str) -> i32 {
    let outcome = process_remember_output(
        kg,
        RememberOutput {
            facts: vec![ExtractedFact {
                classification: Classification::Explicit,
                subject: "Devansh".to_string(),
                subject_type: "Person".to_string(),
                relationship_type: "allergy".to_string(),
                object: object.to_string(),
                object_is_entity: false,
                object_type: None,
                temporal: None,
                is_sensitive: true,
                correction_scope: None,
                categories: vec!["230".to_string()],
            }],
        },
    )
    .await
    .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.pending_confirmation[0].fact_id
}

#[tokio::test]
async fn pending_confirmation_cleanup_uses_configured_retention() {
    // A fact 5 days old is stale under a 3-day retention but would survive the
    // old hardcoded 7-day window, so this proves the pass reads the config.
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, clock, _dir) = pending_kg_with_clock(start).await;
    let stale_id = insert_pending_fact(&kg, "peanuts").await;

    // Advance 5 days: older than 3-day retention, younger than the old 7-day default.
    clock.advance(Duration::days(5));

    let runner = OptimizationRunner::new(
        &kg,
        OptimizationConfig {
            backup_dir: std::path::PathBuf::from("/tmp/mimir-test-backups"),
            timeout_minutes: 120,
            schedule_time: "02:00".to_string(),
            pending_cleanup_retention_days: 3,
        },
        None,
    );

    let summary = runner
        .run_pass(PassName::PendingConfirmationCleanup)
        .await
        .unwrap();
    assert_eq!(summary.facts_forgotten, 1);
    assert!(kg.get_fact(stale_id).await.unwrap().is_none());
}

#[tokio::test]
async fn pending_confirmation_cleanup_skips_facts_within_retention_window() {
    let start = DateTime::parse_from_rfc3339("2024-03-15T12:00:00Z")
        .unwrap()
        .into();
    let (kg, clock, _dir) = pending_kg_with_clock(start).await;
    let fresh_id = insert_pending_fact(&kg, "shellfish").await;

    // Only 2 days old under a 3-day retention: must survive.
    clock.advance(Duration::days(2));

    let runner = OptimizationRunner::new(
        &kg,
        OptimizationConfig {
            backup_dir: std::path::PathBuf::from("/tmp/mimir-test-backups"),
            timeout_minutes: 120,
            schedule_time: "02:00".to_string(),
            pending_cleanup_retention_days: 3,
        },
        None,
    );

    let summary = runner
        .run_pass(PassName::PendingConfirmationCleanup)
        .await
        .unwrap();
    assert_eq!(summary.facts_forgotten, 0);
    assert!(kg.get_fact(fresh_id).await.unwrap().is_some());
}
