//! Tests for relationship-type DAG subtree fact queries (issue #134).
//!
//! Verifies that `get_facts_by_relationship_subtree` walks the
//! `relationship_type_hierarchy` DAG via a SQLite recursive CTE and returns
//! facts whose relationship type is the root or any descendant, filtered by
//! fact status (`NOT IN (5, 6)`), pending confirmation, confidence threshold,
//! and limit, sorted by confidence descending. Also covers the
//! `KgQueryTool` `include_subtree` parameter.
//!
//! Custom relationship types are prefixed with `kb134_` to avoid collisions
//! with the seeded taxonomy and its aliases (e.g. `graduated_from` is a seeded
//! alias of `studied_at`, `interests` is a seeded alias of `hobby`).

use chrono::{Duration, Utc};
use mimir_core::tools::{Tool, ToolError};
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries::fact::{
    count_facts_by_relationship_subtree, get_facts_by_relationship_subtree, set_status,
};
use std::sync::Arc;

mod common;

/// Insert one `UserEdit` fact for `subject` with predicate `predicate`, an
/// object entity, and an explicit confidence. Returns the new fact id.
async fn add_fact(
    tg: &common::TestGraph,
    subject: i32,
    predicate: &str,
    object: Option<i32>,
    confidence: f32,
) -> i32 {
    let new_fact = NewFact {
        subject_id: subject,
        relationship_type: predicate.to_string(),
        object_id: object,
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(confidence),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    let relationship_type_id = tg.kg.ensure_relationship_type(predicate).await.unwrap();
    let canonical_name = tg.kg.relationship_type_name(relationship_type_id).await;
    let mut new_fact = new_fact;
    new_fact.relationship_type = canonical_name.unwrap_or_else(|| predicate.to_string());
    mimir_knowledge::queries::fact::insert_fact(
        tg.kg.pool(),
        &new_fact,
        relationship_type_id,
        confidence,
        Utc::now(),
    )
    .await
    .unwrap()
    .id
}

/// Ensure a relationship type exists and return its id.
async fn rt(tg: &common::TestGraph, name: &str) -> i16 {
    tg.kg.ensure_relationship_type(name).await.unwrap()
}

/// Add a hierarchy edge: `child` becomes a descendant of `parent`.
async fn edge(tg: &common::TestGraph, child: i16, parent: i16) {
    tg.kg
        .insert_relationship_type_hierarchy(child, parent)
        .await
        .unwrap();
}

/// Collect fact ids from enriched rows.
fn ids_of(facts: &[mimir_knowledge::queries::fact::FactWithSources]) -> Vec<i32> {
    facts.iter().map(|f| f.id).collect()
}

#[tokio::test]
async fn subtree_returns_root_and_all_descendants_sorted_by_confidence() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let oxford = tg.create_place("Oxford").await;

    // education -> studied_at -> is_enrolled; education -> graduated_from
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    let is_enrolled = rt(&tg, "kb134_is_enrolled").await;
    let graduated_from = rt(&tg, "kb134_graduated_from").await;
    edge(&tg, studied_at, education).await;
    edge(&tg, is_enrolled, studied_at).await;
    edge(&tg, graduated_from, education).await;

    add_fact(&tg, alice, "kb134_education", Some(oxford), 0.50).await;
    add_fact(&tg, alice, "kb134_studied_at", Some(oxford), 0.90).await;
    add_fact(&tg, alice, "kb134_is_enrolled", Some(oxford), 0.70).await;
    add_fact(&tg, alice, "kb134_graduated_from", Some(oxford), 0.80).await;
    // Unrelated predicate must be excluded even though high confidence.
    add_fact(&tg, alice, "kb134_unrelated", Some(oxford), 0.99).await;

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 50)
        .await
        .unwrap();

    assert_eq!(facts.len(), 4, "root + 3 descendants, unrelated excluded");
    assert_eq!(facts[0].confidence, 0.90);
    assert_eq!(facts[1].confidence, 0.80);
    assert_eq!(facts[2].confidence, 0.70);
    assert_eq!(facts[3].confidence, 0.50);
}

#[tokio::test]
async fn subtree_dedups_facts_reachable_via_multiple_paths() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;

    // Diamond: spatial -> located, located -> is_in, spatial -> is_in.
    let root = rt(&tg, "kb134_spatial").await;
    let mid = rt(&tg, "kb134_located").await;
    let leaf = rt(&tg, "kb134_is_in").await;
    edge(&tg, mid, root).await;
    edge(&tg, leaf, mid).await;
    edge(&tg, leaf, root).await;

    add_fact(&tg, alice, "kb134_spatial", Some(ox), 0.40).await;
    add_fact(&tg, alice, "kb134_located", Some(ox), 0.60).await;
    add_fact(&tg, alice, "kb134_is_in", Some(ox), 0.90).await;

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, root, 0.0, 50)
        .await
        .unwrap();
    assert_eq!(
        facts.len(),
        3,
        "each type's fact appears once despite diamond"
    );
    let leaf_count = facts.iter().filter(|f| f.confidence == 0.90).count();
    assert_eq!(leaf_count, 1, "leaf fact deduplicated across two paths");
}

#[tokio::test]
async fn subtree_status_filter_excludes_superseded_keeps_others() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;

    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    let graduated_from = rt(&tg, "kb134_graduated_from").await;
    let completed_degree = rt(&tg, "kb134_completed_degree").await;
    let enrolled_at = rt(&tg, "kb134_enrolled_at").await;
    edge(&tg, studied_at, education).await;
    edge(&tg, graduated_from, education).await;
    edge(&tg, completed_degree, education).await;
    edge(&tg, enrolled_at, education).await;

    let active_id = add_fact(&tg, alice, "kb134_education", Some(ox), 0.50).await;
    let disputed_id = add_fact(&tg, alice, "kb134_studied_at", Some(ox), 0.60).await;
    let inferred_id = add_fact(&tg, alice, "kb134_graduated_from", Some(ox), 0.70).await;
    let corrected_id = add_fact(&tg, alice, "kb134_completed_degree", Some(ox), 0.80).await;
    let superseded_id = add_fact(&tg, alice, "kb134_enrolled_at", Some(ox), 0.90).await;

    let now = Utc::now();
    set_status(
        tg.kg.pool(),
        disputed_id,
        FactStatus::Disputed,
        now,
        ChangedBy::System,
    )
    .await
    .unwrap();
    set_status(
        tg.kg.pool(),
        inferred_id,
        FactStatus::Inferred,
        now,
        ChangedBy::System,
    )
    .await
    .unwrap();
    set_status(
        tg.kg.pool(),
        corrected_id,
        FactStatus::Corrected,
        now,
        ChangedBy::System,
    )
    .await
    .unwrap();
    set_status(
        tg.kg.pool(),
        superseded_id,
        FactStatus::Superseded,
        now,
        ChangedBy::System,
    )
    .await
    .unwrap();

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 50)
        .await
        .unwrap();
    let ids = ids_of(&facts);
    assert!(ids.contains(&active_id), "Active fact included");
    assert!(ids.contains(&disputed_id), "Disputed fact included");
    assert!(ids.contains(&inferred_id), "Inferred-status fact included");
    assert!(ids.contains(&corrected_id), "Corrected fact included");
    assert!(!ids.contains(&superseded_id), "Superseded fact excluded");
    assert_eq!(facts.len(), 4);
}

#[tokio::test]
async fn subtree_excludes_forgotten_and_pending_facts() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;

    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    let graduated_from = rt(&tg, "kb134_graduated_from").await;
    edge(&tg, studied_at, education).await;
    edge(&tg, graduated_from, education).await;

    let kept_id = add_fact(&tg, alice, "kb134_education", Some(ox), 0.80).await;
    let forgotten_id = add_fact(&tg, alice, "kb134_studied_at", Some(ox), 0.90).await;
    let pending_id = add_fact(&tg, alice, "kb134_graduated_from", Some(ox), 0.70).await;

    tg.kg
        .forget_fact(forgotten_id, ChangedBy::System)
        .await
        .unwrap();
    sqlx::query("UPDATE facts SET pending_confirmation = TRUE WHERE id = ?")
        .bind(pending_id)
        .execute(tg.kg.pool())
        .await
        .unwrap();

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 50)
        .await
        .unwrap();
    let ids = ids_of(&facts);
    assert!(ids.contains(&kept_id), "kept fact returned");
    assert!(!ids.contains(&forgotten_id), "forgotten fact absent");
    assert!(!ids.contains(&pending_id), "pending fact excluded");
    assert_eq!(facts.len(), 1);
}

#[tokio::test]
async fn subtree_respects_limit_and_keeps_highest_confidence() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;

    let types = ["kb134_t1", "kb134_t2", "kb134_t3", "kb134_t4", "kb134_t5"];
    let confidences = [0.10_f32, 0.20, 0.30, 0.40, 0.50];
    for (t, c) in types.iter().zip(confidences.iter()) {
        let id = rt(&tg, t).await;
        edge(&tg, id, education).await;
        add_fact(&tg, alice, t, Some(ox), *c).await;
    }

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 2)
        .await
        .unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].confidence, 0.50);
    assert_eq!(facts[1].confidence, 0.40);
}

#[tokio::test]
async fn subtree_respects_min_confidence_and_count() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    let graduated_from = rt(&tg, "kb134_graduated_from").await;
    edge(&tg, studied_at, education).await;
    edge(&tg, graduated_from, education).await;
    add_fact(&tg, alice, "kb134_education", Some(ox), 0.30).await;
    add_fact(&tg, alice, "kb134_studied_at", Some(ox), 0.60).await;
    add_fact(&tg, alice, "kb134_graduated_from", Some(ox), 0.90).await;

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.5, 50)
        .await
        .unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].confidence, 0.90);
    assert_eq!(facts[1].confidence, 0.60);

    let count = count_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.5)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn subtree_returns_multiple_facts_of_same_descendant_type() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let chess = tg.create_activity("Chess").await;
    let hiking = tg.create_activity("Hiking").await;

    // `likes` is a multi-valued predicate and is not part of the seeded taxonomy.
    let interests = rt(&tg, "kb134_interests").await;
    let likes = rt(&tg, "likes").await;
    edge(&tg, likes, interests).await;
    add_fact(&tg, alice, "likes", Some(chess), 0.80).await;
    add_fact(&tg, alice, "likes", Some(hiking), 0.70).await;

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, interests, 0.0, 50)
        .await
        .unwrap();
    assert_eq!(facts.len(), 2, "both multi-valued likes facts returned");
}

#[tokio::test]
async fn subtree_returns_empty_when_no_matching_facts() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    edge(&tg, studied_at, education).await;

    // A fact exists but for a different subject.
    let bob = tg.create_person("Bob").await;
    add_fact(&tg, bob, "kb134_studied_at", Some(ox), 0.90).await;

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 50)
        .await
        .unwrap();
    assert!(
        facts.is_empty(),
        "no facts for Alice in the education subtree"
    );

    let none = get_facts_by_relationship_subtree(tg.kg.pool(), 99999, education, 0.0, 50)
        .await
        .unwrap();
    assert!(none.is_empty(), "nonexistent subject returns empty");
}

#[tokio::test]
async fn subtree_preserves_temporal_bounds_in_output() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    edge(&tg, studied_at, education).await;

    let from = Utc::now() - Duration::days(365);
    let until = Utc::now();
    let new_fact = NewFact {
        subject_id: alice,
        relationship_type: "kb134_studied_at".to_string(),
        object_id: Some(ox),
        object_literal: None,
        valid_from: Some(from),
        valid_until: Some(until),
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.80),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    let canonical_name = tg.kg.relationship_type_name(studied_at).await;
    let mut new_fact = new_fact;
    new_fact.relationship_type = canonical_name.unwrap_or_default();
    mimir_knowledge::queries::fact::insert_fact(
        tg.kg.pool(),
        &new_fact,
        studied_at,
        0.80,
        Utc::now(),
    )
    .await
    .unwrap();

    let facts = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 50)
        .await
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].valid_from, Some(from));
    assert_eq!(facts[0].valid_until, Some(until));
}

#[tokio::test]
async fn subtree_wrapper_matches_free_function() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    edge(&tg, studied_at, education).await;
    add_fact(&tg, alice, "kb134_education", Some(ox), 0.50).await;
    add_fact(&tg, alice, "kb134_studied_at", Some(ox), 0.90).await;

    let via_wrapper = tg
        .kg
        .get_facts_by_relationship_subtree(alice, education, 50)
        .await
        .unwrap();
    let via_free = get_facts_by_relationship_subtree(tg.kg.pool(), alice, education, 0.0, 50)
        .await
        .unwrap();
    assert_eq!(via_wrapper.len(), via_free.len());
    assert_eq!(ids_of(&via_wrapper), ids_of(&via_free));
}

// ---------------------------------------------------------------------------
// KgQueryTool: include_subtree parameter
// ---------------------------------------------------------------------------

fn fact_predicates(result: &serde_json::Value) -> Vec<String> {
    result["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["predicate"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn kg_query_include_subtree_returns_descendant_facts() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    let graduated_from = rt(&tg, "kb134_graduated_from").await;
    edge(&tg, studied_at, education).await;
    edge(&tg, graduated_from, education).await;
    add_fact(&tg, alice, "kb134_education", Some(ox), 0.50).await;
    add_fact(&tg, alice, "kb134_studied_at", Some(ox), 0.90).await;
    add_fact(&tg, alice, "kb134_graduated_from", Some(ox), 0.80).await;
    // Unrelated, high-confidence fact must be excluded by the subtree.
    add_fact(&tg, alice, "kb134_unrelated", Some(ox), 0.99).await;

    let tool = mimir_knowledge::KgQueryTool::new(Arc::new(tg.kg));
    let out = tool
        .execute(serde_json::json!({
            "entity_name": "Alice",
            "predicate": "kb134_education",
            "include_subtree": true,
            "offset": 25,
        }))
        .await
        .unwrap();
    let result = out.result.unwrap();
    let facts = result["facts"].as_array().unwrap();
    assert_eq!(facts.len(), 3, "education + studied_at + graduated_from");
    assert_eq!(result["total"].as_i64().unwrap(), 3);
    assert_eq!(
        result["offset"].as_i64().unwrap(),
        0,
        "subtree mode forces offset=0 and ignores input offset"
    );
    let preds = fact_predicates(&result);
    assert!(preds.contains(&"kb134_education".to_string()));
    assert!(preds.contains(&"kb134_studied_at".to_string()));
    assert!(preds.contains(&"kb134_graduated_from".to_string()));
    assert!(!preds.contains(&"kb134_unrelated".to_string()));
    // Sorted by confidence descending.
    let confs: Vec<f64> = facts
        .iter()
        .map(|f| f["confidence"].as_f64().unwrap())
        .collect();
    assert!(confs[0] > confs[1], "descending confidence order");
}

#[tokio::test]
async fn kg_query_include_subtree_resolves_alias_for_root() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    // `studied_at` is canonical; `attended` is a seeded alias (migration 036).
    let studied_at = rt(&tg, "studied_at").await;
    let enrolled = rt(&tg, "kb134_enrolled").await;
    edge(&tg, enrolled, studied_at).await;
    add_fact(&tg, alice, "studied_at", Some(ox), 0.90).await;
    add_fact(&tg, alice, "kb134_enrolled", Some(ox), 0.70).await;

    let tool = mimir_knowledge::KgQueryTool::new(Arc::new(tg.kg));
    let out = tool
        .execute(serde_json::json!({
            "entity_name": "Alice",
            "predicate": "attended",
            "include_subtree": true,
        }))
        .await
        .unwrap();
    let result = out.result.unwrap();
    let facts = result["facts"].as_array().unwrap();
    assert_eq!(
        facts.len(),
        2,
        "alias resolves to studied_at, subtree includes kb134_enrolled"
    );
}

#[tokio::test]
async fn kg_query_include_subtree_uses_seeded_employment_parent() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let engineering = tg.create_activity("Engineering").await;
    // The seeded employment parent (issue #403) must expand to its children.
    add_fact(&tg, alice, "works_at", Some(engineering), 0.90).await;
    add_fact(&tg, alice, "works_as", Some(engineering), 0.80).await;
    add_fact(&tg, alice, "job_title", Some(engineering), 0.70).await;
    // Unrelated, high-confidence fact must be excluded by the subtree.
    add_fact(&tg, alice, "hobby", Some(engineering), 0.99).await;

    let tool = mimir_knowledge::KgQueryTool::new(Arc::new(tg.kg));
    let out = tool
        .execute(serde_json::json!({
            "entity_name": "Alice",
            "predicate": "employment",
            "include_subtree": true,
        }))
        .await
        .unwrap();
    let result = out.result.unwrap();
    let facts = result["facts"].as_array().unwrap();
    assert_eq!(
        facts.len(),
        3,
        "employment subtree: works_at + works_as + job_title"
    );
    let preds = fact_predicates(&result);
    assert!(preds.contains(&"works_at".to_string()));
    assert!(preds.contains(&"works_as".to_string()));
    assert!(preds.contains(&"job_title".to_string()));
    assert!(!preds.contains(&"hobby".to_string()));
}

#[tokio::test]
async fn kg_query_without_include_subtree_uses_exact_predicate() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    let education = rt(&tg, "kb134_education").await;
    let studied_at = rt(&tg, "kb134_studied_at").await;
    edge(&tg, studied_at, education).await;
    add_fact(&tg, alice, "kb134_education", Some(ox), 0.50).await;
    add_fact(&tg, alice, "kb134_studied_at", Some(ox), 0.90).await;

    let tool = mimir_knowledge::KgQueryTool::new(Arc::new(tg.kg));
    let out = tool
        .execute(serde_json::json!({
            "entity_name": "Alice",
            "predicate": "kb134_education",
        }))
        .await
        .unwrap();
    let result = out.result.unwrap();
    let facts = result["facts"].as_array().unwrap();
    assert_eq!(
        facts.len(),
        1,
        "exact predicate match, descendants excluded"
    );
    assert_eq!(facts[0]["predicate"].as_str().unwrap(), "kb134_education");
}

#[tokio::test]
async fn kg_query_include_subtree_unknown_predicate_returns_empty() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let ox = tg.create_place("Oxford").await;
    add_fact(&tg, alice, "kb134_unrelated", Some(ox), 0.90).await;

    let tool = mimir_knowledge::KgQueryTool::new(Arc::new(tg.kg));
    let out = tool
        .execute(serde_json::json!({
            "entity_name": "Alice",
            "predicate": "nonexistent_root",
            "include_subtree": true,
            "offset": 25,
        }))
        .await
        .unwrap();
    let result = out.result.unwrap();
    assert!(result["facts"].as_array().unwrap().is_empty());
    assert_eq!(result["total"].as_i64().unwrap(), 0);
    assert_eq!(
        result["offset"].as_i64().unwrap(),
        0,
        "subtree mode forces offset=0 even when no facts match"
    );
}

#[tokio::test]
async fn kg_query_include_subtree_without_predicate_is_error() {
    let tg = common::TestGraph::new().await;
    let tool = mimir_knowledge::KgQueryTool::new(Arc::new(tg.kg));
    let result = tool
        .execute(serde_json::json!({
            "entity_name": "Alice",
            "include_subtree": true,
        }))
        .await;
    assert!(
        matches!(result, Err(ToolError::InvalidArguments { .. })),
        "include_subtree without a predicate must be rejected, got {:?}",
        result
    );
}
