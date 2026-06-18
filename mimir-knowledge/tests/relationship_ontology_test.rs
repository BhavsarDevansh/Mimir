//! Tests for the core relationship ontology seed (#135, category-first).
//!
//! Predicates are thin canonical verbs; verb synonyms live in
//! `relationship_type_aliases`. This verifies the seeded predicate count,
//! alias resolution, and idempotency.

use mimir_knowledge::KnowledgeGraph;
use std::path::PathBuf;

async fn setup() -> (tempfile::TempDir, KnowledgeGraph) {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();
    (dir, kg)
}

#[tokio::test]
async fn seed_creates_expected_predicate_count() {
    let (_dir, kg) = setup().await;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relationship_types")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(count, 31, "31 canonical relationship types after seed");

    // The six new core verbs are present with their explicit ids.
    for (id, name) in [
        (26i16, "studied"),
        (27, "completed_degree"),
        (28, "educational_status"),
        (29, "job_title"),
        (30, "likes"),
        (31, "dislikes"),
    ] {
        let (db_name,): (String,) =
            sqlx::query_as("SELECT name FROM relationship_types WHERE id = ?")
                .bind(id)
                .fetch_one(kg.pool())
                .await
                .unwrap();
        assert_eq!(db_name, name);
    }
}

#[tokio::test]
async fn new_predicates_have_self_aliases() {
    let (_dir, kg) = setup().await;
    for name in [
        "studied",
        "completed_degree",
        "educational_status",
        "job_title",
        "likes",
        "dislikes",
    ] {
        let resolved = kg.ensure_relationship_type(name).await.unwrap();
        let alias_id = kg.resolve_relationship_type_alias(name).await.unwrap();
        assert_eq!(alias_id, Some(resolved), "{name} self-alias missing");
    }
}

#[tokio::test]
async fn legacy_verb_aliases_resolve_to_canonical() {
    let (_dir, kg) = setup().await;
    let studied_at = kg.ensure_relationship_type("studied_at").await.unwrap();
    let has_partner = kg.ensure_relationship_type("has_partner").await.unwrap();

    assert_eq!(
        kg.resolve_relationship_type_alias("attended")
            .await
            .unwrap(),
        Some(studied_at)
    );
    assert_eq!(
        kg.resolve_relationship_type_alias("went_to").await.unwrap(),
        Some(studied_at)
    );
    assert_eq!(
        kg.resolve_relationship_type_alias("graduated_from")
            .await
            .unwrap(),
        Some(studied_at)
    );
    assert_eq!(
        kg.resolve_relationship_type_alias("alumni_of")
            .await
            .unwrap(),
        Some(studied_at)
    );
    assert_eq!(
        kg.resolve_relationship_type_alias("wife").await.unwrap(),
        Some(has_partner)
    );

    // Case- and whitespace-insensitive resolution.
    assert_eq!(
        kg.resolve_relationship_type_alias("  Attended  ")
            .await
            .unwrap(),
        Some(studied_at)
    );
}

#[tokio::test]
async fn seed_is_idempotent_across_reinit() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();
    let (types_before,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relationship_types")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    let (aliases_before,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM relationship_type_aliases")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    drop(kg);

    // Re-initialising the same database must not duplicate seeded rows.
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();
    let (types_after,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relationship_types")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    let (aliases_after,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM relationship_type_aliases")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(types_before, types_after);
    assert_eq!(aliases_before, aliases_after);
}
