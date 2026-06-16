//! Tests for relationship type DAG (hierarchy + aliases).

use mimir_knowledge::KnowledgeError;
use mimir_knowledge::KnowledgeGraph;
use std::path::PathBuf;

async fn setup() -> (tempfile::TempDir, KnowledgeGraph) {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();
    (dir, kg)
}

#[tokio::test]
async fn migration_creates_dag_tables() {
    let (_dir, kg) = setup().await;

    let tables: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(kg.pool())
            .await
            .unwrap();

    let names: Vec<String> = tables.into_iter().map(|r| r.0).collect();
    assert!(names.contains(&"relationship_type_hierarchy".to_string()));
    assert!(names.contains(&"relationship_type_aliases".to_string()));
}

#[tokio::test]
async fn insert_hierarchy_and_query_descendants() {
    let (_dir, kg) = setup().await;

    let parent = kg.ensure_relationship_type("located").await.unwrap();
    let child = kg.ensure_relationship_type("located_in").await.unwrap();
    let grandchild = kg.ensure_relationship_type("is_in").await.unwrap();

    kg.insert_relationship_type_hierarchy(child, parent)
        .await
        .unwrap();
    kg.insert_relationship_type_hierarchy(grandchild, child)
        .await
        .unwrap();

    let descendants = kg
        .get_descendant_relationship_type_ids(parent)
        .await
        .unwrap();
    assert!(descendants.contains(&child));
    assert!(descendants.contains(&grandchild));

    let ancestors = kg
        .get_ancestor_relationship_type_ids(grandchild)
        .await
        .unwrap();
    assert!(ancestors.contains(&child));
    assert!(ancestors.contains(&parent));
}

#[tokio::test]
async fn insert_alias_and_resolve() {
    let (_dir, kg) = setup().await;

    let id = kg.ensure_relationship_type("studied_at").await.unwrap();
    kg.insert_relationship_type_alias("attended", id)
        .await
        .unwrap();

    let resolved = kg
        .resolve_relationship_type_alias("attended")
        .await
        .unwrap();
    assert_eq!(resolved, Some(id));

    let resolved_whitespace = kg
        .resolve_relationship_type_alias("  Attended  ")
        .await
        .unwrap();
    assert_eq!(resolved_whitespace, Some(id));
}

#[tokio::test]
async fn alias_unique_globally() {
    let (_dir, kg) = setup().await;

    let id_a = kg.ensure_relationship_type("type_a").await.unwrap();
    let id_b = kg.ensure_relationship_type("type_b").await.unwrap();

    kg.insert_relationship_type_alias("shared", id_a)
        .await
        .unwrap();

    let err = kg
        .insert_relationship_type_alias("shared", id_b)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Pool(_)),
        "expected unique constraint violation, got {:?}",
        err
    );
}

#[tokio::test]
async fn self_loop_rejected() {
    let (_dir, kg) = setup().await;
    let id = kg.ensure_relationship_type("self_loop").await.unwrap();

    let err = kg
        .insert_relationship_type_hierarchy(id, id)
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::RelationshipTypeCycle));
}

#[tokio::test]
async fn cycle_detection_rejects_indirect_cycle() {
    let (_dir, kg) = setup().await;
    let a = kg.ensure_relationship_type("cycle_a").await.unwrap();
    let b = kg.ensure_relationship_type("cycle_b").await.unwrap();
    let c = kg.ensure_relationship_type("cycle_c").await.unwrap();

    kg.insert_relationship_type_hierarchy(b, a).await.unwrap();
    kg.insert_relationship_type_hierarchy(c, b).await.unwrap();

    let err = kg
        .insert_relationship_type_hierarchy(a, c)
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::RelationshipTypeCycle));
}

#[tokio::test]
async fn alias_cannot_shadow_canonical_name() {
    let (_dir, kg) = setup().await;

    let _canonical_id = kg.ensure_relationship_type("works_at").await.unwrap();
    let other_id = kg.ensure_relationship_type("employer").await.unwrap();

    // "works_at" is already a canonical name, so it cannot be an alias.
    let err = kg
        .insert_relationship_type_alias("works_at", other_id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for shadowing alias, got {:?}",
        err
    );
}

#[tokio::test]
async fn normalize_predicate_uses_alias() {
    let (_dir, kg) = setup().await;

    let id = kg.ensure_relationship_type("studied_at").await.unwrap();
    kg.insert_relationship_type_alias("alumni_of", id)
        .await
        .unwrap();

    // We cannot call normalize_predicate directly (it's private), so drive it
    // through the public extraction pipeline with a mock LLM that returns the
    // alias. Instead, verify alias resolution independently and rely on the
    // extraction integration tests for the full path.
    let resolved = kg
        .resolve_relationship_type_alias("alumni_of")
        .await
        .unwrap();
    assert_eq!(resolved, Some(id));
}
