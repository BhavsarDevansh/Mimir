//! Tests for relationship type DAG (hierarchy + aliases).

use mimir_knowledge::KnowledgeError;
use mimir_knowledge::KnowledgeGraph;

mod common;
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

    let parent = common::ensure_relationship_type(&kg, "located")
        .await
        .unwrap();
    let child = common::ensure_relationship_type(&kg, "located_in")
        .await
        .unwrap();
    // `is_in` is a seeded alias of `located_in` (migration 051), so use a
    // fresh name for the grandchild to keep three distinct types.
    let grandchild = common::ensure_relationship_type(&kg, "is_contained_in")
        .await
        .unwrap();

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

    let root = common::ensure_relationship_type(&kg, "spatial")
        .await
        .unwrap();
    let mid = common::ensure_relationship_type(&kg, "located")
        .await
        .unwrap();
    let leaf = common::ensure_relationship_type(&kg, "is_in")
        .await
        .unwrap();

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
async fn seeded_dag_expresses_domain_generalisation() {
    let (_dir, kg) = setup().await;

    // Employment subtree: works_at, works_as, job_title (issue #403).
    let employment = kg
        .get_relationship_type_id("employment")
        .await
        .unwrap()
        .expect("employment parent must be seeded");
    let descendants = kg
        .get_descendant_relationship_type_ids(employment)
        .await
        .unwrap();
    for child in ["works_at", "works_as", "job_title"] {
        let id = kg
            .get_relationship_type_id(child)
            .await
            .unwrap()
            .expect("employment child must be seeded");
        assert!(
            descendants.contains(&id),
            "{child} must be a descendant of employment"
        );
    }

    // Education subtree: studied, studied_at, completed_degree, educational_status.
    let education = kg
        .get_relationship_type_id("education")
        .await
        .unwrap()
        .expect("education parent must be seeded");
    let descendants = kg
        .get_descendant_relationship_type_ids(education)
        .await
        .unwrap();
    for child in [
        "studied",
        "studied_at",
        "completed_degree",
        "educational_status",
    ] {
        let id = kg
            .get_relationship_type_id(child)
            .await
            .unwrap()
            .expect("education child must be seeded");
        assert!(
            descendants.contains(&id),
            "{child} must be a descendant of education"
        );
    }

    // Residence subtree: the consolidated resides_in verb.
    let residence = kg
        .get_relationship_type_id("residence")
        .await
        .unwrap()
        .expect("residence parent must be seeded");
    let descendants = kg
        .get_descendant_relationship_type_ids(residence)
        .await
        .unwrap();
    let resides_in = kg
        .get_relationship_type_id("resides_in")
        .await
        .unwrap()
        .expect("resides_in must be seeded");
    assert!(
        descendants.contains(&resides_in),
        "resides_in must be a descendant of residence"
    );

    // Containment subtree: located_in (is_in consolidated into it).
    let containment = kg
        .get_relationship_type_id("containment")
        .await
        .unwrap()
        .expect("containment parent must be seeded");
    let descendants = kg
        .get_descendant_relationship_type_ids(containment)
        .await
        .unwrap();
    let located_in = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .expect("located_in must be seeded");
    assert!(
        descendants.contains(&located_in),
        "located_in must be a descendant of containment"
    );
}

#[tokio::test]
async fn consolidated_predicates_resolve_as_aliases() {
    let (_dir, kg) = setup().await;

    // based_in / lived_in are aliases of the single resides_in verb.
    let resides_in = kg
        .get_relationship_type_id("resides_in")
        .await
        .unwrap()
        .expect("resides_in must be seeded");
    for alias in ["based_in", "lived_in"] {
        let id = kg.resolve_relationship_type_alias(alias).await.unwrap();
        assert_eq!(id, Some(resides_in), "{alias} must resolve to resides_in");
    }

    // is_in is an alias of located_in.
    let located_in = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .expect("located_in must be seeded");
    let is_in = kg.resolve_relationship_type_alias("is_in").await.unwrap();
    assert_eq!(is_in, Some(located_in), "is_in must resolve to located_in");

    // The old rows are gone: no canonical row named based_in/lived_in/is_in.
    for gone in ["based_in", "lived_in", "is_in"] {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM relationship_types WHERE name = ?")
                .bind(gone)
                .fetch_one(kg.pool())
                .await
                .unwrap();
        assert_eq!(count, 0, "{gone} must no longer be a canonical row");
    }
}

#[tokio::test]
async fn empty_alias_is_rejected() {
    let (_dir, kg) = setup().await;
    let id = common::ensure_relationship_type(&kg, "has_quality")
        .await
        .unwrap();

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

    let id = common::ensure_relationship_type(&kg, "studied_at")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("test_attended_alias", id)
        .await
        .unwrap();

    let resolved = kg
        .resolve_relationship_type_alias("test_attended_alias")
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
async fn test_fixture_resolves_alias_instead_of_conflicting() {
    let (_dir, kg) = setup().await;

    let existing_id = common::ensure_relationship_type(&kg, "works_at")
        .await
        .unwrap();
    let alias_id = kg
        .resolve_relationship_type_alias("employer")
        .await
        .unwrap()
        .expect("seeded works_for alias");
    assert_eq!(alias_id, existing_id);

    // "test_employer_alias" is an alias, so the test fixture resolves it to the
    // canonical type rather than creating a new one or failing.
    let resolved_id = common::ensure_relationship_type(&kg, "employer")
        .await
        .unwrap();
    assert_eq!(resolved_id, existing_id);
}

#[tokio::test]
async fn alias_unique_globally() {
    let (_dir, kg) = setup().await;

    let id_a = common::ensure_relationship_type(&kg, "type_a")
        .await
        .unwrap();
    let id_b = common::ensure_relationship_type(&kg, "type_b")
        .await
        .unwrap();

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
    let id = common::ensure_relationship_type(&kg, "self_loop")
        .await
        .unwrap();

    let err = kg
        .insert_relationship_type_hierarchy(id, id)
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::RelationshipTypeCycle));
}

#[tokio::test]
async fn cycle_detection_rejects_indirect_cycle() {
    let (_dir, kg) = setup().await;
    let a = common::ensure_relationship_type(&kg, "cycle_a")
        .await
        .unwrap();
    let b = common::ensure_relationship_type(&kg, "cycle_b")
        .await
        .unwrap();
    let c = common::ensure_relationship_type(&kg, "cycle_c")
        .await
        .unwrap();

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

    let parent_id = common::ensure_relationship_type(&kg, "located")
        .await
        .unwrap();
    let rt = mimir_knowledge::models::relationship_type::NewRelationshipType {
        name: "contained_in".to_string(),
        description: None,
        sensitive: false,
        default_memory_priority_id: None,
        parent_ids: vec![parent_id],
        aliases: vec!["inside".to_string(), "within".to_string()],
    };
    let inserted = kg.insert_relationship_type(rt).await.unwrap();
    assert_eq!(inserted.name, "contained_in");
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

    let _canonical_id = common::ensure_relationship_type(&kg, "works_at")
        .await
        .unwrap();
    let other_id = common::ensure_relationship_type(&kg, "test_employer_alias")
        .await
        .unwrap();

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
async fn alias_resolution_returns_canonical_id() {
    let (_dir, kg) = setup().await;

    let id = common::ensure_relationship_type(&kg, "studied_at")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("test_alumni_alias", id)
        .await
        .unwrap();

    // The alias table is the single source of truth for predicate resolution
    // (issue #136). This test uses `mimir-test-support::ensure_relationship_type`
    // as a fixture helper to exercise alias resolution; production extraction
    // routes through `resolve_canonical_relationship_type`.
    let resolved = kg
        .resolve_relationship_type_alias("test_alumni_alias")
        .await
        .unwrap();
    assert_eq!(resolved, Some(id));
}

#[tokio::test]
async fn insert_relationship_type_rejects_canonical_name_that_shadows_alias() {
    let (_dir, kg) = setup().await;

    let _existing_id = common::ensure_relationship_type(&kg, "works_at")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("test_employer_alias", _existing_id)
        .await
        .unwrap();

    let rt = mimir_knowledge::models::relationship_type::NewRelationshipType {
        name: "test_employer_alias".to_string(),
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

    let _canonical_id = common::ensure_relationship_type(&kg, "works_at")
        .await
        .unwrap();

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
async fn transactional_fact_insert_resolves_relationship_type_alias() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::SourceType;

    let (_dir, kg) = setup().await;

    let existing_id = common::ensure_relationship_type(&kg, "works_at")
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
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: vec![],
        category_ids: vec![],
    };

    let facts = kg.insert_facts_batch(vec![fact]).await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].relationship_type_id, existing_id);
}

#[tokio::test]
async fn test_fixture_resolves_alias_to_canonical() {
    let (_dir, kg) = setup().await;
    let canonical_id = common::ensure_relationship_type(&kg, "studied_at")
        .await
        .unwrap();
    kg.insert_relationship_type_alias("test_attended_alias", canonical_id)
        .await
        .unwrap();

    let resolved_id = common::ensure_relationship_type(&kg, "test_attended_alias")
        .await
        .unwrap();
    assert_eq!(
        resolved_id, canonical_id,
        "the test fixture should resolve 'attended' alias to canonical 'studied_at'"
    );
    assert_eq!(
        kg.get_relationship_type_id("test_attended_alias")
            .await
            .unwrap(),
        Some(canonical_id),
        "get_relationship_type_id should also resolve the alias"
    );
}

#[tokio::test]
async fn test_fixture_creates_new_type_and_self_alias() {
    let (_dir, kg) = setup().await;
    let id = common::ensure_relationship_type(&kg, "foo_bar")
        .await
        .unwrap();

    let name_id = kg.get_relationship_type_id("foo_bar").await.unwrap();
    assert_eq!(
        name_id,
        Some(id),
        "new canonical name should resolve to the created type"
    );

    let aliases: Vec<String> = sqlx::query_scalar(
        "SELECT alias FROM relationship_type_aliases WHERE relationship_type_id = ?",
    )
    .bind(id)
    .fetch_all(kg.pool())
    .await
    .unwrap();
    assert!(
        aliases.contains(&"foo_bar".to_string()),
        "new canonical type should register its normalized name as a self-alias"
    );
}

#[tokio::test]
async fn existing_relationship_type_priority_is_not_replaced_by_upsert() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::relationship_type::NewRelationshipType;
    use mimir_knowledge::models::source::SourceType;

    let (_dir, kg) = setup().await;
    let created = kg
        .insert_relationship_type(NewRelationshipType {
            name: "test_cached_priority".to_string(),
            description: None,
            sensitive: false,
            default_memory_priority_id: Some(2),
            parent_ids: vec![],
            aliases: vec![],
        })
        .await
        .unwrap();

    let _updated = kg
        .insert_relationship_type(NewRelationshipType {
            name: "test_cached_priority".to_string(),
            description: None,
            sensitive: false,
            default_memory_priority_id: Some(4),
            parent_ids: vec![],
            aliases: vec![],
        })
        .await
        .unwrap();

    let subject = kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();
    let fact = mimir_knowledge::queries::fact::insert_fact(
        kg.pool(),
        &NewFact {
            subject_id: subject.id,
            relationship_type: "test_cached_priority".to_string(),
            object_id: None,
            object_literal: Some("unbounded".to_string()),
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
            parent_fact_ids: vec![],
            category_ids: vec![],
        },
        created.id,
        0.80,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    assert_eq!(fact.relationship_type_id, created.id);
    assert_eq!(fact.memory_priority_id, 2);
}
