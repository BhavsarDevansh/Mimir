//! Tests that all migrations run cleanly and produce the expected schema.

use mimir_knowledge::KnowledgeGraph;
use std::path::PathBuf;

#[tokio::test]
async fn all_migrations_apply_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let tables: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(kg.pool())
            .await
            .unwrap();

    let names: Vec<String> = tables.into_iter().map(|r| r.0).collect();

    // Core tables
    assert!(names.contains(&"entities".to_string()));
    assert!(names.contains(&"entity_aliases".to_string()));
    assert!(names.contains(&"entity_dates".to_string()));
    assert!(names.contains(&"entity_locations".to_string()));
    assert!(names.contains(&"facts".to_string()));
    assert!(names.contains(&"fact_dependencies".to_string()));
    assert!(names.contains(&"sources".to_string()));
    assert!(names.contains(&"preferences".to_string()));
    assert!(names.contains(&"preference_sources".to_string()));
    assert!(names.contains(&"preference_contexts".to_string()));
    assert!(names.contains(&"preference_audit_log".to_string()));
    assert!(names.contains(&"fact_audit_log".to_string()));
    assert!(names.contains(&"dedup_queue".to_string()));
    assert!(names.contains(&"entity_merge_queue".to_string()));
    assert!(names.contains(&"trash".to_string()));
    assert!(names.contains(&"system_state".to_string()));

    // Lookup tables
    assert!(names.contains(&"entity_types".to_string()));
    assert!(names.contains(&"entity_date_types".to_string()));
    assert!(names.contains(&"recurrence_types".to_string()));
    assert!(names.contains(&"location_types".to_string()));
    assert!(names.contains(&"fact_statuses".to_string()));
    assert!(names.contains(&"relation_types".to_string()));
    assert!(names.contains(&"source_types".to_string()));
    assert!(names.contains(&"preference_categories".to_string()));
    assert!(names.contains(&"preference_source_types".to_string()));
    assert!(names.contains(&"predicates".to_string()));
    assert!(names.contains(&"predicate_constraints".to_string()));
    assert!(names.contains(&"extraction_methods".to_string()));
    assert!(names.contains(&"change_types".to_string()));
    assert!(names.contains(&"changed_by_types".to_string()));
    assert!(names.contains(&"connector_types".to_string()));
}

#[tokio::test]
async fn lookup_tables_seeded_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let queries: Vec<(&'static str, i64)> = vec![
        ("SELECT COUNT(*) FROM entity_types", 8),
        ("SELECT COUNT(*) FROM entity_date_types", 6),
        ("SELECT COUNT(*) FROM recurrence_types", 5),
        ("SELECT COUNT(*) FROM location_types", 5),
        ("SELECT COUNT(*) FROM fact_statuses", 6),
        ("SELECT COUNT(*) FROM relation_types", 4),
        ("SELECT COUNT(*) FROM source_types", 6),
        ("SELECT COUNT(*) FROM preference_categories", 7),
        ("SELECT COUNT(*) FROM preference_source_types", 3),
        ("SELECT COUNT(*) FROM predicates", 12),
    ];

    for (query, expected) in queries {
        let (count,): (i64,) = sqlx::query_as(query).fetch_one(kg.pool()).await.unwrap();
        assert_eq!(count, expected, "{} should return {}", query, expected);
    }
}

#[tokio::test]
async fn fts5_virtual_table_exists() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let (name,): (String,) = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'entity_fts'",
    )
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(name, "entity_fts");
}

#[tokio::test]
async fn wal_and_foreign_keys_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let (journal_mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(journal_mode.to_lowercase(), "wal");

    let (fk_enabled,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(fk_enabled, 1);
}
