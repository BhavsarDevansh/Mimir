//! Tool-call fact extraction integration tests (Issue #55).

use std::sync::Arc;

use mimir_core::llm::MockLlmClient;
use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};

use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::source::SourceType;

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

// ---------------------------------------------------------------------------
// Test 1: Explicit extraction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_explicit_extraction() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "favourite_colour",
        "object": "blue",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "My favourite colour is blue.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());
    assert!(outcome.errors.is_empty());

    let fact = &outcome.inserted[0];
    assert_eq!(fact.confidence, 1.0);
    assert_eq!(fact.status(), Some(FactStatus::Active));
    assert!(!fact.pending_confirmation);

    let sources = tg.kg.get_sources_for_fact(fact.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::UserEdit as i16);
}

// ---------------------------------------------------------------------------
// Test 2: Casual extraction (lower confidence, no overwrite)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_casual_extraction() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;
    let auckland = tg.create_place("Auckland").await;

    // Pre-insert an explicit based_in fact (single-valued predicate).
    tg.create_fact(devansh, "based_in", Some(auckland), SourceType::UserEdit)
        .await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Casual",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "based_in",
        "object": "London",
        "object_is_entity": true,
        "object_type": "Place",
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "London is a nice city.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let fact = &outcome.inserted[0];
    assert_eq!(fact.confidence, 0.30);
    assert_eq!(fact.status(), Some(FactStatus::Disputed));

    // The old explicit fact should still exist but be Disputed due to casual contradiction.
    let old_facts = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let explicit = old_facts.iter().find(|f| f.confidence == 1.0).unwrap();
    assert_eq!(explicit.status(), Some(FactStatus::Disputed));
}

// ---------------------------------------------------------------------------
// Test 3: Entity resolution (existing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_resolution_existing() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "likes",
        "object": "coding",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "devansh likes coding.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(outcome.inserted[0].subject_id, devansh);
}

// ---------------------------------------------------------------------------
// Test 4: Entity creation (new)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_creation_new() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "alice",
        "subject_type": "Person",
        "relationship_type": "works_as",
        "object": "engineer",
        "object_is_entity": false,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "Alice works as an engineer.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);

    let alice = tg
        .kg
        .get_entity(outcome.inserted[0].subject_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice.name, "alice");
    assert_eq!(alice.entity_type_id, EntityType::Person as i16);
}

// ---------------------------------------------------------------------------
// Test 5: Temporal correction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_temporal_correction() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    // Pre-insert an open-ended fact.
    tg.create_fact_with_temporal(devansh, "based_in", None, None, None, SourceType::UserEdit)
        .await;

    let now = chrono::Utc::now();
    let scope = now.to_rfc3339();

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Correction",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "based_in",
        "object": "Manchester",
        "object_is_entity": false,
        "correction_scope": scope,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "As of now I live in Manchester.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let new_fact = &outcome.inserted[0];
    assert_eq!(new_fact.status(), Some(FactStatus::Active));

    // Old fact should have been closed.
    let facts = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let old = facts.iter().find(|f| f.id != new_fact.id).unwrap();
    assert!(old.valid_until.is_some());
    assert_eq!(old.status(), Some(FactStatus::Superseded));
}

// ---------------------------------------------------------------------------
// Test 5b: Correction with no scope defaults to a temporal correction at now
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_correction_no_scope_defaults_to_temporal_at_now() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    // Pre-insert an open-ended fact.
    tg.create_fact_with_temporal(devansh, "based_in", None, None, None, SourceType::UserEdit)
        .await;

    // The LLM emits a Correction classification but omits correction_scope
    // (which it is told to set but may not). The boundary must still treat
    // this as a temporal correction at `now`, superseding the predecessor.
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Correction",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "based_in",
        "object": "Manchester",
        "object_is_entity": false,
        "correction_scope": null,
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "Actually I live in Manchester now.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let new_fact = &outcome.inserted[0];
    assert_eq!(new_fact.status(), Some(FactStatus::Active));
    // The boundary defaulted the correction to a temporal correction at now.
    assert!(new_fact.valid_from.is_some());

    // Old fact should have been closed at the new fact's valid_from.
    let facts = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let old = facts.iter().find(|f| f.id != new_fact.id).unwrap();
    assert!(old.valid_until.is_some());
    assert_eq!(old.status(), Some(FactStatus::Superseded));
}

// ---------------------------------------------------------------------------
// Test 6: Retrospective correction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retrospective_correction() {
    let tg = TestGraph::new().await;
    let devansh = tg.create_person("devansh").await;

    // Pre-insert an active fact.
    let old = tg
        .create_fact(devansh, "favourite_colour", None, SourceType::UserEdit)
        .await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Correction",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "favourite_colour",
        "object": "green",
        "object_is_entity": false,
        "correction_scope": "always",
        "is_sensitive": false
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(
            &mock,
            "My favourite colour has always been green, not blue.",
        )
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let new_fact = &outcome.inserted[0];
    assert_eq!(new_fact.status(), Some(FactStatus::Active));
    assert_eq!(new_fact.confidence, 1.0);

    // Old fact should be in trash.
    let old_fact = tg.kg.get_fact(old.id).await.unwrap();
    assert!(old_fact.is_none());
}

// ---------------------------------------------------------------------------
// Test 7: Sensitive fact confirmation flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sensitive_fact_confirmation() {
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

    let pending = &outcome.pending_confirmation[0];
    assert_eq!(pending.relationship_type, "allergy");
    assert_eq!(pending.object_display, "peanuts");

    // Verify DB state.
    let fact = tg.kg.get_fact(pending.fact_id).await.unwrap().unwrap();
    assert_eq!(fact.status(), Some(FactStatus::Disputed));
    assert!(fact.pending_confirmation);

    // Confirm the fact.
    let confirmed = tg.kg.confirm_fact(pending.fact_id).await.unwrap();
    assert_eq!(confirmed.status(), Some(FactStatus::Active));
    assert_eq!(confirmed.confidence, 1.0);
    assert!(!confirmed.pending_confirmation);

    // Verify cache is clear.
    assert!(
        !tg.kg
            .pending_confirmations()
            .read()
            .await
            .contains(&pending.fact_id)
    );
}

// ---------------------------------------------------------------------------
// Test 8: Multiple facts in one message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_facts() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![
        serde_json::json!({
            "classification": "Explicit",
            "subject": "devansh",
            "subject_type": "Person",
            "relationship_type": "favourite_colour",
            "object": "blue",
            "object_is_entity": false,
            "is_sensitive": false
        }),
        serde_json::json!({
            "classification": "Casual",
            "subject": "devansh",
            "subject_type": "Person",
            "relationship_type": "likes",
            "object": "pizza",
            "object_is_entity": false,
            "is_sensitive": false
        }),
    ]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "My favourite colour is blue. I also like pizza.")
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 2);
    assert!(outcome.pending_confirmation.is_empty());
    assert!(outcome.errors.is_empty());
}

// ---------------------------------------------------------------------------
// Test 9: Invalid LLM output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_invalid_llm_output() {
    let tg = TestGraph::new().await;

    let msg = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCall {
            index: 0,
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: "not json".to_string(),
            },
        }]),
        tool_call_id: None,
    };

    let mock: Arc<dyn mimir_core::llm::backend::LlmBackend> = Arc::new(
        MockLlmClient::builder()
            .push_chat_message(msg, Usage::default())
            .build(),
    );

    let result = tg.kg.extract_facts(&mock, "This is a test.").await;

    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Test 10: Empty extraction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_empty_extraction() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg.kg.extract_facts(&mock, "Hello!").await.unwrap();

    assert!(outcome.inserted.is_empty());
    assert!(outcome.pending_confirmation.is_empty());
    assert!(outcome.errors.is_empty());
}

// ---------------------------------------------------------------------------
// Test 11: Reject sensitive fact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reject_sensitive_fact() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "allergy",
        "object": "shellfish",
        "object_is_entity": false,
        "is_sensitive": true,
        "categories": ["230"]
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I am allergic to shellfish.")
        .await
        .unwrap();

    let pending = &outcome.pending_confirmation[0];

    // Reject the fact.
    tg.kg.reject_fact(pending.fact_id, None).await.unwrap();

    // Verify deletion.
    assert!(tg.kg.get_fact(pending.fact_id).await.unwrap().is_none());
    assert!(
        !tg.kg
            .pending_confirmations()
            .read()
            .await
            .contains(&pending.fact_id)
    );
}

// ---------------------------------------------------------------------------
// Test 12: Pending confirmation 7-day TTL cleanup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pending_confirmation_ttl_cleanup() {
    let tg = TestGraph::new().await;

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "test_user",
        "subject_type": "Person",
        "relationship_type": "health_condition",
        "object": "diabetes",
        "object_is_entity": false,
        "is_sensitive": true
    })]);

    let mock = build_mock_with_tool_output(tool_args);
    let outcome = tg
        .kg
        .extract_facts(&mock, "I have diabetes.")
        .await
        .unwrap();

    assert_eq!(outcome.pending_confirmation.len(), 1);
    let pending = &outcome.pending_confirmation[0];
    let fact_id = pending.fact_id;

    // Verify the fact exists and is in the cache.
    let fact = tg.kg.get_fact(fact_id).await.unwrap().unwrap();
    assert!(fact.pending_confirmation);
    assert!(
        tg.kg
            .pending_confirmations()
            .read()
            .await
            .contains(&fact_id)
    );

    // Backdate the fact to 8 days ago (both created_at and updated_at) so it
    // exceeds the 7-day pending retention window.
    let eight_days_ago = chrono::Utc::now() - chrono::Duration::days(8);
    sqlx::query("UPDATE facts SET created_at = ?, updated_at = ? WHERE id = ?")
        .bind(eight_days_ago)
        .bind(eight_days_ago)
        .bind(fact_id)
        .execute(tg.kg.pool())
        .await
        .unwrap();

    // Run the nightly optimization which includes the cleanup.
    mimir_knowledge::optimization::run_nightly_optimization(&tg.kg, &tg.backup_dir())
        .await
        .unwrap();

    // Verify the fact has been hard-deleted.
    assert!(tg.kg.get_fact(fact_id).await.unwrap().is_none());

    // Verify it's removed from the in-memory cache.
    assert!(
        !tg.kg
            .pending_confirmations()
            .read()
            .await
            .contains(&fact_id)
    );
}

#[tokio::test]
async fn test_alias_resolution_attended_to_studied_at() {
    let tg = TestGraph::new().await;
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "Devansh",
        "subject_type": "Person",
        "relationship_type": "attended",
        "object": "University of Auckland",
        "object_is_entity": false,
        "categories": [],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(&mock, "I attended University of Auckland.")
        .await
        .unwrap();

    assert_eq!(result.inserted.len(), 1);
    let fact = &result.inserted[0];
    let pred = tg
        .kg
        .relationship_type_name(fact.relationship_type_id)
        .await;
    assert_eq!(pred.as_deref(), Some("studied_at"));
    assert_eq!(
        fact.object_literal.as_deref(),
        Some("University of Auckland")
    );
}

#[tokio::test]
async fn test_alias_resolution_uses_db_alias() {
    let tg = TestGraph::new().await;

    // Seed an alias so "matriculated_at" resolves to the canonical "studied_at".
    // Deliberately use a synonym that is *not* in the deprecated hardcoded map.
    let studied_at_id = common::ensure_relationship_type(&tg.kg, "studied_at")
        .await
        .unwrap();
    tg.kg
        .insert_relationship_type_alias("matriculated_at", studied_at_id)
        .await
        .unwrap();

    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "Devansh",
        "subject_type": "Person",
        "relationship_type": "matriculated_at",
        "object": "University of Auckland",
        "object_is_entity": false,
        "categories": [],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(&mock, "I am an alumni of University of Auckland.")
        .await
        .unwrap();

    assert_eq!(result.inserted.len(), 1);
    let fact = &result.inserted[0];
    let pred = tg
        .kg
        .relationship_type_name(fact.relationship_type_id)
        .await;
    assert_eq!(pred.as_deref(), Some("studied_at"));
}

#[tokio::test]
async fn test_split_hobbies_into_individual_facts() {
    let tg = TestGraph::new().await;
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "Devansh",
        "subject_type": "Person",
        "relationship_type": "hobbies",
        "object": "Geopolitics, Software Development, Tech",
        "object_is_entity": false,
        "categories": [],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(
            &mock,
            "My hobbies are Geopolitics, Software Development, and Tech.",
        )
        .await
        .unwrap();

    assert_eq!(
        result.inserted.len(),
        3,
        "expected 3 hobby facts, got {:?}",
        result.inserted
    );
    let preds: Vec<Option<String>> = futures::future::join_all(
        result
            .inserted
            .iter()
            .map(|f| tg.kg.relationship_type_name(f.relationship_type_id)),
    )
    .await;
    for p in &preds {
        assert_eq!(p.as_deref(), Some("hobby"));
    }
    let objects: Vec<Option<&str>> = result
        .inserted
        .iter()
        .map(|f| f.object_literal.as_deref())
        .collect();
    assert!(objects.contains(&Some("Geopolitics")));
    assert!(objects.contains(&Some("Software Development")));
    assert!(objects.contains(&Some("Tech")));
}

#[tokio::test]
async fn test_split_favourite_family_into_individual_facts() {
    let tg = TestGraph::new().await;
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "Devansh",
        "subject_type": "Person",
        "relationship_type": "prefers",
        "object": "Inception, Interstellar",
        "object_is_entity": false,
        "categories": [],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(&mock, "My favourite movies are Inception and Interstellar.")
        .await
        .unwrap();

    assert_eq!(
        result.inserted.len(),
        2,
        "expected 2 prefers facts, got {:?}",
        result.inserted
    );
    let objects: Vec<Option<&str>> = result
        .inserted
        .iter()
        .map(|f| f.object_literal.as_deref())
        .collect();
    assert!(objects.contains(&Some("Inception")));
    assert!(objects.contains(&Some("Interstellar")));

    // The open `favourite_<thing>` family must carry multi-valued insert
    // semantics too: re-reading from the DB, both split facts must still be
    // active rather than the second superseding the first.
    let active = tg
        .kg
        .get_active_facts_at(
            result.inserted[0].subject_id,
            result.inserted[0].relationship_type_id,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let active_objects: Vec<Option<&str>> =
        active.iter().map(|f| f.object_literal.as_deref()).collect();
    assert_eq!(active_objects.len(), 2, "got {active_objects:?}");
    assert!(active_objects.contains(&Some("Inception")));
    assert!(active_objects.contains(&Some("Interstellar")));
}

#[tokio::test]
async fn test_preferred_name_creates_alias() {
    let tg = TestGraph::new().await;

    // Seed the canonical entity
    let canonical = tg
        .kg
        .create_entity("Devansh Bhavsar", EntityType::Person, &[])
        .await
        .unwrap();

    // Simulate the LLM extracting a preferred_name fact
    let tool_args = make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "Devansh Bhavsar",
        "subject_type": "Person",
        "relationship_type": "preferred_name",
        "object": "Devansh",
        "object_is_entity": false,
        "categories": ["110"],
    })]);
    let mock = build_mock_with_tool_output(tool_args);

    let result = tg
        .kg
        .extract_facts(&mock, "I go by Devansh.")
        .await
        .unwrap();

    assert_eq!(result.inserted.len(), 1);

    // The alias should now exist, so get_by_name("Devansh") resolves to the canonical entity
    let resolved = mimir_knowledge::queries::entity::get_by_name(tg.kg.pool(), "Devansh")
        .await
        .unwrap();
    assert!(!resolved.is_empty());
    assert_eq!(resolved[0].entity.id, canonical.id);
    assert_eq!(
        resolved[0].match_kind,
        mimir_knowledge::queries::entity::MatchKind::ExactAlias
    );
}
