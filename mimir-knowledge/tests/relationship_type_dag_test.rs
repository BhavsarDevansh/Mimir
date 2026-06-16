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
async fn dag_with_multiple_parents_deduplicates_reachable_ids() {
    let (_dir, kg) = setup().await;

    let root = kg.ensure_relationship_type("spatial").await.unwrap();
    let mid = kg.ensure_relationship_type("located").await.unwrap();
    let leaf = kg.ensure_relationship_type("is_in").await.unwrap();

    // leaf has two parents: root and mid, and mid is also a child of root.
    kg.insert_relationship_type_hierarchy(mid, root)
        .await
        .unwrap();
    kg.insert_relationship_type_hierarchy(leaf, mid)
        .await
        .unwrap();
    kg.insert_relationship_type_hierarchy(leaf, root)
        .await
        .unwrap();

    let descendants = kg.get_descendant_relationship_type_ids(root).await.unwrap();
    assert_eq!(
        descendants.iter().filter(|&&id| id == leaf).count(),
        1,
        "leaf reachable via two paths must appear once"
    );
    assert!(descendants.contains(&mid));
    assert!(descendants.contains(&leaf));
}

#[tokio::test]
async fn empty_alias_is_rejected() {
    let (_dir, kg) = setup().await;
    let id = kg.ensure_relationship_type("has_quality").await.unwrap();

    let err = kg
        .insert_relationship_type_alias("   ", id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for empty alias, got {:?}",
        err
    );

    let resolved = kg.resolve_relationship_type_alias("   ").await.unwrap();
    assert_eq!(resolved, None);
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
async fn canonical_name_cannot_shadow_alias() {
    let (_dir, kg) = setup().await;

    let existing_id = kg
        .ensure_relationship_type("employer_entity")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("employer", existing_id)
        .await
        .unwrap();

    // Creating a canonical relationship type called "employer" should now fail
    // because the alias already occupies that name.
    let err = kg.ensure_relationship_type("employer").await.unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for canonical name shadowing alias, got {:?}",
        err
    );
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
async fn get_relationship_type_includes_parents_and_aliases() {
    let (_dir, kg) = setup().await;

    let parent_id = kg.ensure_relationship_type("located").await.unwrap();
    let rt = mimir_knowledge::models::relationship_type::NewRelationshipType {
        name: "is_in".to_string(),
        description: None,
        sensitive: false,
        default_memory_priority_id: None,
        parent_ids: vec![parent_id],
        aliases: vec!["inside".to_string(), "within".to_string()],
    };
    let inserted = kg.insert_relationship_type(rt).await.unwrap();
    assert_eq!(inserted.name, "is_in");
    assert_eq!(inserted.parent_ids, vec![parent_id]);
    assert!(inserted.aliases.contains(&"inside".to_string()));
    assert!(inserted.aliases.contains(&"within".to_string()));

    let loaded = kg
        .get_relationship_type(inserted.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.parent_ids, vec![parent_id]);
    assert!(loaded.aliases.contains(&"inside".to_string()));
    assert!(loaded.aliases.contains(&"within".to_string()));
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

#[tokio::test]
async fn insert_relationship_type_rejects_canonical_name_that_shadows_alias() {
    let (_dir, kg) = setup().await;

    let existing_id = kg
        .ensure_relationship_type("employer_entity")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("employer", existing_id)
        .await
        .unwrap();

    let rt = mimir_knowledge::models::relationship_type::NewRelationshipType {
        name: "employer".to_string(),
        description: None,
        sensitive: false,
        default_memory_priority_id: None,
        parent_ids: vec![],
        aliases: vec![],
    };
    let err = kg.insert_relationship_type(rt).await.unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for canonical name shadowing alias, got {:?}",
        err
    );
}

#[tokio::test]
async fn insert_relationship_type_rejects_alias_that_shadows_canonical_name() {
    let (_dir, kg) = setup().await;

    let _canonical_id = kg.ensure_relationship_type("works_at").await.unwrap();

    let rt = mimir_knowledge::models::relationship_type::NewRelationshipType {
        name: "new_type".to_string(),
        description: None,
        sensitive: false,
        default_memory_priority_id: None,
        parent_ids: vec![],
        aliases: vec!["works_at".to_string()],
    };
    let err = kg.insert_relationship_type(rt).await.unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for alias shadowing canonical name, got {:?}",
        err
    );
}

#[tokio::test]
async fn transactional_fact_insert_rejects_relationship_type_name_that_shadows_alias() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::SourceType;

    let (_dir, kg) = setup().await;

    let existing_id = kg
        .ensure_relationship_type("employer_entity")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("employer", existing_id)
        .await
        .unwrap();

    let entity = kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();

    let fact = NewFact {
        subject_id: entity.id,
        relationship_type: "employer".to_string(),
        object_id: None,
        object_literal: Some("Acme Corp".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: vec![],
        category_ids: vec![],
    };

    let err = kg.insert_facts_batch(vec![fact]).await.unwrap_err();
    assert!(
        matches!(err, KnowledgeError::Validation(_)),
        "expected validation error for transactional create shadowing alias, got {:?}",
        err
    );
}
