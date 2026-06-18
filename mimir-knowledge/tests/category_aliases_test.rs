//! Tests for category aliases and category-subtree retrieval (#135).

use mimir_knowledge::KnowledgeError;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::source::SourceType;
use std::path::PathBuf;

async fn setup() -> (tempfile::TempDir, KnowledgeGraph) {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();
    (dir, kg)
}

#[tokio::test]
async fn category_aliases_table_seeded() {
    let (_dir, kg) = setup().await;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM category_aliases")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert!(
        count >= 24,
        "at least 24 seeded category aliases, got {count}"
    );
}

#[tokio::test]
async fn resolve_domain_aliases_to_dewey_nodes() {
    let (_dir, kg) = setup().await;
    let cases = [
        ("education", 550),
        ("schooling", 550),
        ("employment", 510),
        ("residence", 610),
        ("hobbies", 770),
        ("interests", 770),
        ("leisure", 700),
        ("pets", 440),
        ("family", 410),
        ("identity", 100),
        ("biography", 100),
    ];
    for (alias, expected) in cases {
        let resolved = kg.resolve_category_alias(alias).await.unwrap();
        assert_eq!(resolved, Some(expected), "alias '{alias}'");
    }
}

#[tokio::test]
async fn resolve_alias_is_case_and_whitespace_insensitive() {
    let (_dir, kg) = setup().await;
    assert_eq!(
        kg.resolve_category_alias("  Hobbies  ").await.unwrap(),
        Some(770)
    );
}

#[tokio::test]
async fn empty_alias_resolves_to_none() {
    let (_dir, kg) = setup().await;
    assert_eq!(kg.resolve_category_alias("   ").await.unwrap(), None);
}

#[tokio::test]
async fn insert_alias_rejects_empty_and_unknown_category() {
    let (_dir, kg) = setup().await;
    let err = kg.insert_category_alias("   ", 550).await.unwrap_err();
    assert!(matches!(err, KnowledgeError::Validation(_)));

    let err = kg
        .insert_category_alias("brand_new_domain", 999_999)
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::CategoryNotFound(_)));
}

#[tokio::test]
async fn insert_alias_is_idempotent() {
    let (_dir, kg) = setup().await;
    kg.insert_category_alias("new_interest_word", 770)
        .await
        .unwrap();
    // Re-inserting the same alias for the same category is a no-op.
    kg.insert_category_alias("new_interest_word", 770)
        .await
        .unwrap();
    assert_eq!(
        kg.resolve_category_alias("new_interest_word")
            .await
            .unwrap(),
        Some(770)
    );

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM category_aliases WHERE alias = 'new_interest_word'")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(count, 1, "idempotent insert must not duplicate");
}

#[tokio::test]
async fn insert_alias_rejects_rebind_to_different_category() {
    let (_dir, kg) = setup().await;
    kg.insert_category_alias("shared_word", 770).await.unwrap();

    // Rebinding an existing alias to a different category must error, not
    // silently keep the original mapping.
    let err = kg
        .insert_category_alias("shared_word", 410)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for rebind, got {err:?}"
    );

    // The original mapping is preserved.
    assert_eq!(
        kg.resolve_category_alias("shared_word").await.unwrap(),
        Some(770)
    );

    let (mapped,): (i64,) =
        sqlx::query_as("SELECT category_id FROM category_aliases WHERE alias = 'shared_word'")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(
        mapped, 770,
        "rebind must not overwrite the existing mapping"
    );
}

#[tokio::test]
async fn list_category_aliases_returns_seeded_rows() {
    let (_dir, kg) = setup().await;
    let all = kg.list_category_aliases(None).await.unwrap();
    assert!(!all.is_empty(), "seeded aliases are present");
    assert!(
        all.iter()
            .any(|a| a.alias == "education" && a.category_id == 550)
    );

    let hobbies = kg.list_category_aliases(Some(770)).await.unwrap();
    assert!(hobbies.iter().all(|a| a.category_id == 770));
    assert!(hobbies.iter().any(|a| a.alias == "hobbies"));
}

#[tokio::test]
async fn descendant_category_ids_walks_the_tree() {
    let (_dir, kg) = setup().await;
    // 700 Entertainment & Leisure has 770 Collecting & Hobbies as a descendant.
    let descendants = kg.get_descendant_category_ids(700).await.unwrap();
    assert!(descendants.contains(&770));
    assert!(descendants.contains(&710));
    assert!(
        !descendants.contains(&700),
        "root excluded from descendants"
    );
}

async fn make_fact_with_category(kg: &KnowledgeGraph, predicate: &str, category_id: i32) -> i32 {
    let subject = kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap()
        .id;
    let object = kg
        .create_entity("Geopolitics", EntityType::Concept, &[])
        .await
        .unwrap()
        .id;
    use mimir_knowledge::models::fact::NewFact;
    let fact = NewFact {
        subject_id: subject,
        relationship_type: predicate.to_string(),
        object_id: Some(object),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: vec![category_id],
    };
    kg.insert_fact(fact).await.unwrap().id
}

#[tokio::test]
async fn facts_in_category_subtree_gather_descendants() {
    let (_dir, kg) = setup().await;
    // A fact tagged with the leaf 770 must be found via the 700 subtree.
    let fact_id = make_fact_with_category(&kg, "hobby", 770).await;

    let facts = kg.get_facts_in_category_subtree(700, 100).await.unwrap();
    let ids: Vec<i32> = facts.iter().map(|f| f.fact_id).collect();
    assert!(
        ids.contains(&fact_id),
        "leaf-category fact found via subtree"
    );

    // A fact in an unrelated category must not leak in.
    let other = make_fact_with_category(&kg, "based_in", 610).await;
    assert!(
        !ids.contains(&other),
        "fact outside the subtree must be excluded"
    );
}
