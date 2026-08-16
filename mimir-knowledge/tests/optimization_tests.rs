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

    // Insert an identical triple directly, bypassing the insert pipeline.
    // Since #79, `insert_fact` corroborates same-claim non-explicit facts at
    // insert time, so live duplicates can no longer coexist. The nightly dedup
    // pass remains a safety net for coexisting duplicates (legacy data, direct
    // writes), which is what we emulate here.
    let now = Utc::now();
    let second_id: i32 = sqlx::query_scalar(
        "INSERT INTO facts \
         (subject_id, relationship_type_id, object_id, object_literal, valid_from, valid_until, \
          confidence, fact_status_id, inferred, inference_depth, pending_confirmation, \
          memory_priority_id, created_at, updated_at) \
         VALUES (?, ?, ?, NULL, NULL, NULL, 0.80, ?, 0, 0, 0, 3, ?, ?) \
         RETURNING id",
    )
    .bind(person)
    .bind(first.relationship_type_id)
    .bind(london)
    .bind(FactStatus::Active as i16)
    .bind(now)
    .bind(now)
    .fetch_one(graph.kg.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sources \
         (fact_id, source_type_id, connector_instance_id, connector_type_id, raw_reference, extracted_at, extraction_method_id) \
         VALUES (?, ?, NULL, NULL, '', ?, NULL)",
    )
    .bind(second_id)
    .bind(SourceType::Import as i16)
    .bind(now)
    .execute(graph.kg.pool())
    .await
    .unwrap();

    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph.backup_dir()),
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
        .bind(second_id)
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
        OptimizationConfig::for_test(graph.backup_dir()),
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
        OptimizationConfig::for_test(graph.backup_dir()),
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
        OptimizationConfig::for_test(graph.backup_dir()),
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
                recurrence: None,
                requires_user_action: None,
                location: None,
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

// ---------------------------------------------------------------------------
// PR #174 review follow-up: confidence_recalc must update the stale root.
// ---------------------------------------------------------------------------

async fn audit_count_for(kg: &KnowledgeGraph, fact_id: i32, change_type_id: i16) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM fact_audit_log WHERE fact_id = ? AND change_type_id = ?",
    )
    .bind(fact_id)
    .bind(change_type_id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    count
}

#[tokio::test]
async fn confidence_recalc_recalculates_stale_inferred_root() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let graph = TestGraph::new().await;
    let person = graph.create_person("Devansh").await;
    let london = graph.create_place("London").await;

    // Parent connector fact at the connector default 0.80.
    let parent = graph
        .create_fact(person, "lives_in", Some(london), SourceType::Connector)
        .await;
    assert!((parent.confidence - 0.80).abs() < 1e-6);

    // Inferred child with a deliberately stale/wrong confidence (0.5) flagged
    // stale, as the nightly pass would find it. Correct value for a single
    // positive parent at depth 1 is 0.80 * 0.8 * 0.6 = 0.384.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts \
         (subject_id, relationship_type_id, object_id, confidence, fact_status_id, \
          inferred, inference_depth, stale_confidence, pending_confirmation) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
    )
    .bind(person)
    .bind(parent.relationship_type_id)
    .bind(london)
    .bind(0.5f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(1i32)
    .bind(true)
    .bind(false)
    .fetch_one(graph.kg.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
         VALUES (?, ?, ?)",
    )
    .bind(parent.id)
    .bind(child.id)
    .bind(1i16) // InferredFrom
    .execute(graph.kg.pool())
    .await
    .unwrap();

    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph.backup_dir()),
        None,
    );
    runner.run_pass(PassName::ConfidenceRecalc).await.unwrap();

    let after = graph.kg.get_fact(child.id).await.unwrap().unwrap();
    assert!(
        (after.confidence - 0.384).abs() < 1e-6,
        "expected 0.384, got {}",
        after.confidence
    );
    assert!(!after.stale_confidence);
    assert_eq!(
        audit_count_for(&graph.kg, child.id, ChangeType::ConfidenceChange as i16).await,
        1
    );
}

#[tokio::test]
async fn confidence_recalc_clears_stale_non_inferred_root_without_audit() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let graph = TestGraph::new().await;
    let person = graph.create_person("Devansh").await;
    let london = graph.create_place("London").await;

    let fact = graph
        .create_fact(person, "lives_in", Some(london), SourceType::Connector)
        .await;
    let original_confidence = fact.confidence;

    // Mark a non-inferred fact stale. Its confidence is structural (not derived
    // from parents), so the recalc pass must only clear the flag.
    sqlx::query("UPDATE facts SET stale_confidence = TRUE WHERE id = ?")
        .bind(fact.id)
        .execute(graph.kg.pool())
        .await
        .unwrap();

    let runner = OptimizationRunner::new(
        &graph.kg,
        OptimizationConfig::for_test(graph.backup_dir()),
        None,
    );
    runner.run_pass(PassName::ConfidenceRecalc).await.unwrap();

    let after = graph.kg.get_fact(fact.id).await.unwrap().unwrap();
    assert!(!after.stale_confidence);
    assert!((after.confidence - original_confidence).abs() < 1e-6);
    assert_eq!(
        audit_count_for(&graph.kg, fact.id, ChangeType::ConfidenceChange as i16).await,
        0
    );
}

// ---------------------------------------------------------------------------
// Regression: concurrent full runs sharing one backup directory (issue #241)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_full_runs_with_shared_backup_dir_do_not_corrupt_each_other() {
    // Regression test for #241: `create_backup` picked its filename with a
    // check-then-act sequence (`try_exists` + `VACUUM INTO`), so two
    // optimization runs sharing a backup directory could select the same file
    // and one would fail under parallel load (the flaky
    // `test_pending_confirmation_ttl_cleanup` failure). The filename
    // reservation must be atomic so concurrent runs never collide.
    let backup_dir = tempfile::tempdir().unwrap();
    for _ in 0..5 {
        let graph1 = TestGraph::new().await;
        let graph2 = TestGraph::new().await;
        let runner1 = OptimizationRunner::new(
            &graph1.kg,
            OptimizationConfig::for_test(backup_dir.path().to_path_buf()),
            None,
        );
        let runner2 = OptimizationRunner::new(
            &graph2.kg,
            OptimizationConfig::for_test(backup_dir.path().to_path_buf()),
            None,
        );
        let (result1, result2) = tokio::join!(runner1.run_all(), runner2.run_all());
        result1.unwrap();
        result2.unwrap();
    }

    // Both runs must have actually written their backups.
    let mut backup_count = 0;
    let mut entries = tokio::fs::read_dir(backup_dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("knowledge-") && name.ends_with(".db") {
            backup_count += 1;
        }
    }
    assert!(
        backup_count >= 2,
        "expected at least 2 backups, found {backup_count}"
    );
}
