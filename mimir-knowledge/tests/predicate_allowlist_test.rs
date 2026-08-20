//! Predicate allow-list enforcement tests (issue #401).
//!
//! The conversational extraction path must reject LLM-invented predicates
//! instead of auto-creating `relationship_types` rows, while seeded canonical
//! predicates, their aliases, and the prompt-instructed `favourite_*` family
//! keep resolving.

use std::sync::Arc;

use mimir_core::llm::MockLlmClient;
use mimir_core::llm::types::{FunctionCall, Message, ToolCall, Usage};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::extract::{
    Classification, ExtractedFact, RememberOutput, process_remember_output,
};

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

fn invented_fact(relationship_type: &str) -> serde_json::Value {
    serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": relationship_type,
        "object": "a suburb",
        "object_is_entity": false,
        "is_sensitive": false
    })
}

// ---------------------------------------------------------------------------
// Chat extraction rejects invented predicates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invented_predicate_rejected_and_no_row_created() {
    let tg = TestGraph::new().await;

    let mock =
        build_mock_with_tool_output(make_remember_tool_output(vec![invented_fact("moved_into")]));
    let outcome = tg
        .kg
        .extract_facts(&mock, "devansh moved into a suburb.")
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert!(outcome.pending_confirmation.is_empty());
    assert_eq!(outcome.errors.len(), 1);
    let message = outcome.errors[0].to_string();
    assert!(
        message.contains("moved_into"),
        "error should name the rejected predicate: {message}"
    );

    // No relationship_types row may be auto-created for the invented predicate.
    assert!(
        tg.kg
            .get_relationship_type_id("moved_into")
            .await
            .unwrap()
            .is_none(),
        "invented predicate must not be auto-created"
    );
}

#[tokio::test]
async fn invented_predicate_does_not_abort_rest_of_batch() {
    let tg = TestGraph::new().await;

    let mock = build_mock_with_tool_output(make_remember_tool_output(vec![
        invented_fact("moved_into"),
        serde_json::json!({
            "classification": "Explicit",
            "subject": "devansh",
            "subject_type": "Person",
            "relationship_type": "favourite_colour",
            "object": "blue",
            "object_is_entity": false,
            "is_sensitive": false
        }),
    ]));
    let outcome = tg
        .kg
        .extract_facts(
            &mock,
            "devansh moved into a suburb. Favourite colour is blue.",
        )
        .await
        .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.inserted[0].object_literal.as_deref(), Some("blue"));
}

// ---------------------------------------------------------------------------
// Seeded predicates, aliases, and the favourite_* family keep working
// ---------------------------------------------------------------------------

#[tokio::test]
async fn seeded_alias_still_resolves_to_canonical() {
    let tg = TestGraph::new().await;

    let mock = build_mock_with_tool_output(make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "attended",
        "object": "University of Auckland",
        "object_is_entity": false,
        "is_sensitive": false
    })]));
    let outcome = tg
        .kg
        .extract_facts(&mock, "devansh attended University of Auckland.")
        .await
        .unwrap();

    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(
        tg.kg
            .relationship_type_name(outcome.inserted[0].relationship_type_id)
            .await
            .as_deref(),
        Some("studied_at")
    );
}

#[tokio::test]
async fn favourite_family_is_prompt_instructed_and_allowed() {
    let tg = TestGraph::new().await;

    let mock = build_mock_with_tool_output(make_remember_tool_output(vec![serde_json::json!({
        "classification": "Explicit",
        "subject": "devansh",
        "subject_type": "Person",
        "relationship_type": "favourite_movie",
        "object": "Inception",
        "object_is_entity": false,
        "is_sensitive": false
    })]));
    let outcome = tg
        .kg
        .extract_facts(&mock, "My favourite movie is Inception.")
        .await
        .unwrap();

    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(
        tg.kg
            .relationship_type_name(outcome.inserted[0].relationship_type_id)
            .await
            .as_deref(),
        Some("favourite_movie")
    );
}

// ---------------------------------------------------------------------------
// Strict resolver unit behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_resolver_accepts_seeded_and_alias() {
    let tg = TestGraph::new().await;

    let studied_at = tg
        .kg
        .resolve_canonical_relationship_type("studied_at")
        .await
        .unwrap();
    let via_alias = tg
        .kg
        .resolve_canonical_relationship_type("attended")
        .await
        .unwrap();
    assert_eq!(studied_at, via_alias);

    // Case- and whitespace-insensitive, like the alias table.
    let spaced = tg
        .kg
        .resolve_canonical_relationship_type("  Attended  ")
        .await
        .unwrap();
    assert_eq!(spaced, studied_at);
}

#[tokio::test]
async fn strict_resolver_rejects_unknown_predicate() {
    let tg = TestGraph::new().await;

    let err = tg
        .kg
        .resolve_canonical_relationship_type("wibbles_at")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("wibbles_at"),
        "error should name the predicate: {err}"
    );
}

#[tokio::test]
async fn strict_resolver_rejects_auto_created_type() {
    let tg = TestGraph::new().await;

    // A connector-style insert auto-creates the type; the strict resolver must
    // still refuse it because it is not part of the canonical seed.
    let auto_id = tg.kg.ensure_relationship_type("moved_into").await.unwrap();
    let err = tg
        .kg
        .resolve_canonical_relationship_type("moved_into")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("moved_into"),
        "error should name the predicate: {err}"
    );

    // The auto-created id is not the canonical set's id for anything.
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM relationship_types WHERE id = ? AND description NOT LIKE 'Auto-created relationship_type: %'",
    )
    .bind(auto_id)
    .fetch_one(tg.kg.pool())
    .await
    .unwrap();
    assert_eq!(count, 0, "auto-created row must not be canonical");
}

#[tokio::test]
async fn strict_resolver_allows_favourite_family() {
    let tg = TestGraph::new().await;

    let id = tg
        .kg
        .resolve_canonical_relationship_type("favourite_movie")
        .await
        .unwrap();
    assert_eq!(
        tg.kg.relationship_type_name(id).await.as_deref(),
        Some("favourite_movie")
    );
}

#[tokio::test]
async fn strict_resolver_rejects_abstract_ontology_parent() {
    let tg = TestGraph::new().await;

    // Abstract DAG parents (employment, education, residence, containment)
    // are query-only vocabulary: they must not be usable as fact predicates.
    for parent in ["employment", "education", "residence", "containment"] {
        let err = tg
            .kg
            .resolve_canonical_relationship_type(parent)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(parent),
            "error should name the predicate: {err}"
        );
    }
}

#[tokio::test]
async fn consolidated_aliases_resolve_to_canonical() {
    let tg = TestGraph::new().await;

    // based_in / lived_in consolidated into resides_in (issue #403).
    let resides_in = tg
        .kg
        .resolve_canonical_relationship_type("resides_in")
        .await
        .unwrap();
    for alias in ["based_in", "lived_in"] {
        let id = tg
            .kg
            .resolve_canonical_relationship_type(alias)
            .await
            .unwrap();
        assert_eq!(id, resides_in, "{alias} must resolve to resides_in");
    }

    // is_in consolidated into located_in (issue #403).
    let located_in = tg
        .kg
        .resolve_canonical_relationship_type("located_in")
        .await
        .unwrap();
    let is_in = tg
        .kg
        .resolve_canonical_relationship_type("is_in")
        .await
        .unwrap();
    assert_eq!(is_in, located_in, "is_in must resolve to located_in");
}

// ---------------------------------------------------------------------------
// Allow-list const is pinned to the seed
// ---------------------------------------------------------------------------

#[test]
fn multi_valued_predicates_are_canonical() {
    for name in mimir_knowledge::MULTI_VALUED_PREDICATES {
        assert!(
            mimir_knowledge::CANONICAL_PREDICATES.contains(name),
            "multi-valued predicate {name} missing from CANONICAL_PREDICATES"
        );
    }
}

#[tokio::test]
async fn canonical_const_matches_seeded_relationship_types() {
    let tg = TestGraph::new().await;

    // Every seeded canonical name must be in the allow-list const. Abstract
    // ontology parents (issue #403) are query-only DAG roots and are excluded.
    let seeded: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM relationship_types WHERE description NOT LIKE 'Auto-created relationship_type: %' AND description NOT LIKE 'Abstract ontology parent%'",
    )
    .fetch_all(tg.kg.pool())
    .await
    .unwrap();
    for name in &seeded {
        assert!(
            mimir_knowledge::CANONICAL_PREDICATES.contains(&name.as_str()),
            "seeded predicate {name} missing from CANONICAL_PREDICATES"
        );
    }

    // Every const entry must resolve to a seeded canonical row.
    for name in mimir_knowledge::CANONICAL_PREDICATES {
        let id = tg
            .kg
            .resolve_canonical_relationship_type(name)
            .await
            .unwrap_or_else(|e| panic!("{name} must resolve: {e}"));
        let (seeded_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM relationship_types WHERE id = ? AND description NOT LIKE 'Auto-created relationship_type: %'",
        )
        .bind(id)
        .fetch_one(tg.kg.pool())
        .await
        .unwrap();
        assert_eq!(seeded_count, 1, "{name} must resolve to a seeded row");
    }
}

// ---------------------------------------------------------------------------
// Reconciliation migration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconciliation_migration_deletes_unreferenced_auto_created_types() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    // Simulate pre-050 pollution: one auto-created type referenced by a fact
    // and one that is pure pollution.
    let with_fact = kg.ensure_relationship_type("moved_into").await.unwrap();
    let without_fact = kg.ensure_relationship_type("wibbles_at").await.unwrap();
    let subject = kg
        .create_entity(
            "devansh",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    kg.insert_fact(mimir_knowledge::models::fact::NewFact {
        subject_id: subject.id,
        relationship_type: "moved_into".to_string(),
        object_id: None,
        object_literal: Some("a suburb".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: mimir_knowledge::models::source::SourceType::UserEdit,
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

    // Re-run migration 050 by removing its record and re-applying the migrator.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 50")
        .execute(kg.pool())
        .await
        .unwrap();
    sqlx::migrate!("src/db/migrations")
        .run(kg.pool())
        .await
        .unwrap();

    // The unreferenced auto-created type is gone; the referenced one is kept
    // (data preservation — semantic mapping is the ontology consolidation's job).
    let (without,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relationship_types WHERE id = ?")
        .bind(without_fact)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(without, 0, "zero-fact auto-created type must be deleted");

    // The delete must cascade to the self-alias: an orphaned alias would
    // resolve to a dangling id and make later connector-path inserts fail
    // with a foreign-key error.
    let (orphan_aliases,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM relationship_type_aliases WHERE relationship_type_id = ?",
    )
    .bind(without_fact)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(
        orphan_aliases, 0,
        "deleted type must not leave orphaned alias rows"
    );

    let (with,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relationship_types WHERE id = ?")
        .bind(with_fact)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(with, 1, "auto-created type with facts must be preserved");
}

#[tokio::test]
async fn strict_resolver_rejects_bare_favourite_prefix() {
    let tg = TestGraph::new().await;

    // `favourite_` with no thing is not a predicate: the open-set family must
    // not auto-create a junk row for an empty suffix.
    let err = tg
        .kg
        .resolve_canonical_relationship_type("favourite_")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("favourite_"),
        "error should name the predicate: {err}"
    );
    assert!(
        tg.kg
            .get_relationship_type_id("favourite_")
            .await
            .unwrap()
            .is_none(),
        "bare favourite_ prefix must not auto-create a row"
    );
}

// ---------------------------------------------------------------------------
// process_remember_output direct path (remember tool)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn remember_tool_path_rejects_invented_predicate() {
    let tg = TestGraph::new().await;

    let outcome = process_remember_output(
        &tg.kg,
        RememberOutput {
            facts: vec![ExtractedFact {
                classification: Classification::Explicit,
                subject: "devansh".to_string(),
                subject_type: "Person".to_string(),
                relationship_type: "moved_into".to_string(),
                object: "a suburb".to_string(),
                object_is_entity: false,
                object_type: None,
                temporal: None,
                is_sensitive: false,
                correction_scope: None,
                categories: Vec::new(),
                recurrence: None,
                requires_user_action: None,
                location: None,
            }],
        },
    )
    .await
    .unwrap();

    assert!(outcome.inserted.is_empty());
    assert_eq!(outcome.errors.len(), 1);
    assert!(
        tg.kg
            .get_relationship_type_id("moved_into")
            .await
            .unwrap()
            .is_none()
    );
}
