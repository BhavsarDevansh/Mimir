//! Deduplication merge, alias flagging, and semantic entity dedup tests.

use std::sync::Arc;

use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};
use mimir_core::llm::{LlmBackend, MockLlmClient};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{AutoCompletePolicy, EventType, LocationType, RecurrenceType};
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;

// ---------------------------------------------------------------------------
// Dedup: exact merge + overlapping alias flagging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dedup_exact_merge() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Create two entities with the same name (case-insensitive)
    let a = kg
        .create_entity("Eve", EntityType::Person, &["Evelyn"])
        .await
        .unwrap();
    let b = kg
        .create_entity("eve", EntityType::Person, &["Evie"])
        .await
        .unwrap();

    // Since create_entity dedups on exact name, b should be the same as a
    assert_eq!(a.id, b.id);

    // To test actual merge, insert two distinct rows manually then merge.
    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert a fact for x so x survives the merge (more facts = survivor)
    kg.insert_fact(NewFact {
        subject_id: x.id,
        relationship_type: "located_in".to_string(),
        object_id: None,
        object_literal: Some("somewhere".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    })
    .await
    .unwrap();

    // Insert a fact referencing y so we can verify FK repointing.
    let located_in_id = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .unwrap();
    sqlx::query("INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)")
        .bind(y.id)
        .bind(located_in_id)
        .bind(x.id)
        .bind(1.0f32)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();

    // Merge y into x
    mimir_knowledge::queries::entity::auto_merge_pair(kg.pool(), x.id, y.id)
        .await
        .unwrap();

    // y should be gone
    let gone = kg.get_entity(y.id).await.unwrap();
    assert!(gone.is_none());

    // Fact should now point to x as subject
    let (subject_id,): (i32,) = sqlx::query_as(
        "SELECT subject_id FROM facts WHERE relationship_type_id = ? AND object_id = ?",
    )
    .bind(located_in_id)
    .bind(x.id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(subject_id, x.id);
}

#[tokio::test]
async fn test_auto_merge_migrates_dates_locations_and_cleans_preferences_queue() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert a fact for x so x survives the merge (more facts = survivor)
    kg.insert_fact(NewFact {
        subject_id: x.id,
        relationship_type: "is_in".to_string(),
        object_id: None,
        object_literal: Some("somewhere".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    })
    .await
    .unwrap();

    // Insert a location for y
    kg.insert_location(
        y.id,
        LocationType::Home,
        Some("456 Oak Ave"),
        Some(40.0),
        Some(-74.0),
        Some("America/New_York"),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Insert a fact for y to serve as source_fact_id
    let fact_y = kg
        .insert_fact(NewFact {
            subject_id: y.id,
            relationship_type: "has_preference".to_string(),
            object_id: None,
            object_literal: Some("pref".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Insert an event overlay on fact_y to verify overlays migrate on merge.
    kg.insert_event(mimir_knowledge::models::event::NewEvent {
        fact_id: fact_y.id,
        entity_id: y.id,
        trigger_date: chrono::Utc::now() + chrono::Duration::days(7),
        recurrence: RecurrenceType::Yearly,
        event_type: EventType::Birthday,
        auto_complete_policy: AutoCompletePolicy::Recurring,
        requires_user_action: false,
    })
    .await
    .unwrap();

    // Insert a preference for y (direct SQL since no helper yet)
    sqlx::query("INSERT INTO preferences (entity_id, category_id, key, value, confidence, source_fact_id) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(y.id)
        .bind(1i16)
        .bind("theme")
        .bind("dark")
        .bind(1.0f32)
        .bind(fact_y.id)
        .execute(kg.pool())
        .await
        .unwrap();

    // Insert a merge-queue entry referencing y as duplicate
    sqlx::query(
        "INSERT INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) VALUES (?, ?, ?)",
    )
    .bind(x.id)
    .bind(y.id)
    .bind(1i16)
    .execute(kg.pool())
    .await
    .unwrap();

    // Merge y into x
    mimir_knowledge::queries::entity::auto_merge_pair(kg.pool(), x.id, y.id)
        .await
        .unwrap();

    // y should be gone
    let gone = kg.get_entity(y.id).await.unwrap();
    assert!(gone.is_none());

    // Event overlay should now belong to x (entity_id repointed to survivor).
    let event = kg.get_event_by_fact(fact_y.id).await.unwrap().unwrap();
    assert_eq!(event.entity_id, x.id);

    // Location should now belong to x
    let locs = kg.get_locations(x.id).await.unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].entity_id, x.id);

    // Preference for y should be removed
    let pref_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM preferences WHERE entity_id = ?")
        .bind(y.id)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(pref_count.0, 0);

    // Merge-queue entry referencing y should be removed
    let queue_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM entity_merge_queue WHERE primary_entity_id = ? OR duplicate_entity_id = ?"
    )
    .bind(y.id)
    .bind(y.id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(queue_count.0, 0);
}

#[tokio::test]
async fn test_dedup_overlapping_alias_flag() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Hank", EntityType::Person, &["Henry"])
        .await
        .unwrap();
    let b = kg
        .create_entity("Henrietta", EntityType::Person, &["Henry"])
        .await
        .unwrap();

    // Flag overlapping aliases
    mimir_knowledge::queries::entity::flag_overlapping_aliases(kg.pool())
        .await
        .unwrap();

    let queue: Vec<(i32, i32, i16)> = sqlx::query_as(
        "SELECT primary_entity_id, duplicate_entity_id, status_id FROM entity_merge_queue",
    )
    .fetch_all(kg.pool())
    .await
    .unwrap();

    assert!(!queue.is_empty());
    let entry = queue
        .iter()
        .find(|e| (e.0 == a.id && e.1 == b.id) || (e.0 == b.id && e.1 == a.id));
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().2, 1); // Pending
}

// ---------------------------------------------------------------------------
// Entity location stubs

// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_find_exact_duplicates_returns_empty_when_no_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // create_entity now enforces case-insensitive uniqueness at the DB level,
    // so "alice" resolves to the existing "Alice" record.
    let a = kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("alice", EntityType::Person, &[])
        .await
        .unwrap();
    assert_eq!(a.id, b.id);

    let dups = mimir_knowledge::queries::entity::find_exact_duplicates(kg.pool())
        .await
        .unwrap();
    assert!(
        dups.is_empty(),
        "Expected no duplicates with DB-level uniqueness"
    );
}

// ---------------------------------------------------------------------------
// Semantic entity dedup (issue #282): LLM evaluation + merge-queue lifecycle
// ---------------------------------------------------------------------------

fn mock_entity_dedup_llm(arguments: String) -> Arc<dyn LlmBackend> {
    Arc::new(
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
                            name: "evaluate_entity_dedup_candidates".to_string(),
                            arguments,
                        },
                    }]),
                    tool_call_id: None,
                },
                Usage::default(),
            )
            .build(),
    )
}

#[tokio::test]
async fn test_enqueue_semantic_dedup_writes_merge_suggestion() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jane Smith", EntityType::Person, &["J. Smith"])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jane Smith-Jones", EntityType::Person, &[])
        .await
        .unwrap();

    let args = format!(
        r#"{{"candidates":[{{"entity_a_id":{},"entity_b_id":{},"suggested_action":"merge","llm_confidence":0.85}}]}}"#,
        a.id, b.id
    );
    let llm = mock_entity_dedup_llm(args);
    let (a_id, b_id) = (a.id, b.id);

    let queued =
        mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![(a, b)], &llm)
            .await
            .unwrap();
    assert_eq!(queued, 1);

    let row: (i32, i32, i16, Option<String>, Option<f32>) = sqlx::query_as(
        "SELECT primary_entity_id, duplicate_entity_id, status_id, suggested_action, llm_confidence \
         FROM entity_merge_queue",
    )
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!((row.0, row.1), (a_id, b_id));
    assert_eq!(row.2, 1); // Pending
    assert_eq!(row.3.as_deref(), Some("merge"));
    assert_eq!(row.4, Some(0.85));
}

#[tokio::test]
async fn test_enqueue_semantic_dedup_skips_pairs_not_in_candidate_set() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jane Smith", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jane Smith-Jones", EntityType::Person, &[])
        .await
        .unwrap();

    // The LLM invents a pair that was never in the candidate list.
    let args = r#"{"candidates":[{"entity_a_id":999,"entity_b_id":1000,"suggested_action":"merge","llm_confidence":0.9}]}"#.to_string();
    let llm = mock_entity_dedup_llm(args);

    let queued =
        mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![(a, b)], &llm)
            .await
            .unwrap();
    assert_eq!(queued, 0);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entity_merge_queue")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_enqueue_semantic_dedup_skips_invalid_action_and_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jane Smith", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jane Smith-Jones", EntityType::Person, &[])
        .await
        .unwrap();

    let args = format!(
        r#"{{"candidates":[
            {{"entity_a_id":{},"entity_b_id":{},"suggested_action":"definitely_merge","llm_confidence":0.9}},
            {{"entity_a_id":{},"entity_b_id":{},"suggested_action":"merge","llm_confidence":1.5}}
        ]}}"#,
        a.id, b.id, a.id, b.id
    );
    let llm = mock_entity_dedup_llm(args);

    let queued =
        mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![(a, b)], &llm)
            .await
            .unwrap();
    assert_eq!(queued, 0);
}

#[tokio::test]
async fn test_enqueue_semantic_dedup_no_duplicate_rows_and_enriches_pending() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jane Smith", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jane Smith-Jones", EntityType::Person, &[])
        .await
        .unwrap();

    let args = format!(
        r#"{{"candidates":[{{"entity_a_id":{},"entity_b_id":{},"suggested_action":"merge","llm_confidence":0.7}}]}}"#,
        a.id, b.id
    );
    let llm = mock_entity_dedup_llm(args.clone());
    let queued = mimir_knowledge::queries::entity::enqueue_semantic_dedup(
        kg.pool(),
        vec![(a.clone(), b.clone())],
        &llm,
    )
    .await
    .unwrap();
    assert_eq!(queued, 1);

    // Second run with a different confidence must not duplicate the row; it
    // enriches the existing pending row instead.
    let args2 = format!(
        r#"{{"candidates":[{{"entity_a_id":{},"entity_b_id":{},"suggested_action":"keep_separate","llm_confidence":0.4}}]}}"#,
        a.id, b.id
    );
    let llm2 = mock_entity_dedup_llm(args2);
    let queued =
        mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![(a, b)], &llm2)
            .await
            .unwrap();
    assert_eq!(queued, 1);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entity_merge_queue")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let row: (Option<String>, Option<f32>) =
        sqlx::query_as("SELECT suggested_action, llm_confidence FROM entity_merge_queue")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(row.0.as_deref(), Some("keep_separate"));
    assert_eq!(row.1, Some(0.4));
}

#[tokio::test]
async fn test_enqueue_semantic_dedup_normalizes_pair_order() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jane Smith", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jane Smith-Jones", EntityType::Person, &[])
        .await
        .unwrap();

    // The LLM returns the pair reversed; the queue must store it ordered so
    // the UNIQUE(primary, duplicate) constraint cannot be bypassed.
    let args = format!(
        r#"{{"candidates":[{{"entity_a_id":{},"entity_b_id":{},"suggested_action":"merge","llm_confidence":0.8}}]}}"#,
        b.id, a.id
    );
    let llm = mock_entity_dedup_llm(args);
    let (a_id, b_id) = (a.id, b.id);

    mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![(a, b)], &llm)
        .await
        .unwrap();

    let row: (i32, i32) =
        sqlx::query_as("SELECT primary_entity_id, duplicate_entity_id FROM entity_merge_queue")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!((row.0, row.1), (a_id, b_id));
}

#[tokio::test]
async fn test_enqueue_semantic_dedup_counts_mirrored_pairs_once() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let a = kg
        .create_entity("Jane Smith", EntityType::Person, &[])
        .await
        .unwrap();
    let b = kg
        .create_entity("Jane Smith-Jones", EntityType::Person, &[])
        .await
        .unwrap();

    // The LLM returns the same pair in both orders; the queue must store one
    // row and count it once.
    let args = format!(
        r#"{{"candidates":[
            {{"entity_a_id":{},"entity_b_id":{},"suggested_action":"merge","llm_confidence":0.8}},
            {{"entity_a_id":{},"entity_b_id":{},"suggested_action":"merge","llm_confidence":0.8}}
        ]}}"#,
        a.id, b.id, b.id, a.id
    );
    let llm = mock_entity_dedup_llm(args);

    let queued =
        mimir_knowledge::queries::entity::enqueue_semantic_dedup(kg.pool(), vec![(a, b)], &llm)
            .await
            .unwrap();
    assert_eq!(queued, 1);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entity_merge_queue")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_merge_queue_apply_merges_entities() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();
    kg.insert_fact(NewFact {
        subject_id: x.id,
        relationship_type: "located_in".to_string(),
        object_id: None,
        object_literal: Some("somewhere".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    })
    .await
    .unwrap();

    let queue_id: i64 = sqlx::query_scalar(
        "INSERT INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(x.id)
    .bind(y.id)
    .bind(1i16)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    mimir_knowledge::queries::entity::apply_merge(kg.pool(), queue_id)
        .await
        .unwrap();

    assert!(kg.get_entity(y.id).await.unwrap().is_none());
    let queue_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM entity_merge_queue WHERE id = ?")
            .bind(queue_id)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(queue_count, 0); // auto_merge_pair clears queue rows for the merged entity
}

#[tokio::test]
async fn test_merge_queue_keep_marks_kept_separate() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();
    let queue_id: i64 = sqlx::query_scalar(
        "INSERT INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(x.id)
    .bind(y.id)
    .bind(1i16)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    mimir_knowledge::queries::entity::keep_merge(kg.pool(), queue_id)
        .await
        .unwrap();

    let row: (i16, Option<i16>, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT status_id, resolution_id, processed_at FROM entity_merge_queue WHERE id = ?",
    )
    .bind(queue_id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(row.0, 3); // Complete
    assert_eq!(row.1, Some(2)); // KeptSeparate
    assert!(row.2.is_some());
}

#[tokio::test]
async fn test_merge_queue_actions_reject_non_pending_entry() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let x = kg
        .create_entity("Frank", EntityType::Person, &[])
        .await
        .unwrap();
    let y = kg
        .create_entity("Grace", EntityType::Person, &[])
        .await
        .unwrap();
    let queue_id: i64 = sqlx::query_scalar(
        "INSERT INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(x.id)
    .bind(y.id)
    .bind(1i16)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    mimir_knowledge::queries::entity::keep_merge(kg.pool(), queue_id)
        .await
        .unwrap();
    let err = mimir_knowledge::queries::entity::keep_merge(kg.pool(), queue_id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not pending"));
    let err = mimir_knowledge::queries::entity::apply_merge(kg.pool(), queue_id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not pending"));
}
