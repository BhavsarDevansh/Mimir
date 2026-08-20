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
    assert!(names.contains(&"events".to_string()));
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
    assert!(names.contains(&"event_types".to_string()));
    assert!(names.contains(&"event_statuses".to_string()));
    assert!(names.contains(&"auto_complete_policies".to_string()));
    assert!(names.contains(&"recurrence_types".to_string()));
    assert!(names.contains(&"location_types".to_string()));
    assert!(names.contains(&"fact_statuses".to_string()));
    assert!(names.contains(&"relation_types".to_string()));
    assert!(names.contains(&"source_types".to_string()));
    assert!(names.contains(&"preference_categories".to_string()));
    assert!(names.contains(&"preference_source_types".to_string()));
    assert!(names.contains(&"relationship_types".to_string()));
    assert!(names.contains(&"category_aliases".to_string()));
    assert!(names.contains(&"relationship_constraints".to_string()));
    assert!(names.contains(&"relationship_type_hierarchy".to_string()));
    assert!(names.contains(&"relationship_type_aliases".to_string()));
    assert!(names.contains(&"categories".to_string()));
    assert!(names.contains(&"fact_categories".to_string()));
    assert!(names.contains(&"extraction_methods".to_string()));
    assert!(names.contains(&"change_types".to_string()));
    assert!(names.contains(&"changed_by_types".to_string()));
    assert!(names.contains(&"connector_types".to_string()));
    assert!(names.contains(&"connector_statuses".to_string()));
    assert!(names.contains(&"connector_auth_states".to_string()));
    assert!(names.contains(&"connectors".to_string()));
}

#[tokio::test]
async fn lookup_tables_seeded_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let queries: Vec<(&'static str, i64)> = vec![
        ("SELECT COUNT(*) FROM entity_types", 8),
        ("SELECT COUNT(*) FROM event_types", 6),
        ("SELECT COUNT(*) FROM event_statuses", 5),
        ("SELECT COUNT(*) FROM auto_complete_policies", 3),
        ("SELECT COUNT(*) FROM recurrence_types", 5),
        ("SELECT COUNT(*) FROM location_types", 6),
        ("SELECT COUNT(*) FROM fact_statuses", 6),
        ("SELECT COUNT(*) FROM relation_types", 4),
        ("SELECT COUNT(*) FROM source_types", 6),
        ("SELECT COUNT(*) FROM preference_categories", 7),
        ("SELECT COUNT(*) FROM preference_source_types", 3),
        ("SELECT COUNT(*) FROM relationship_types", 46),
        ("SELECT COUNT(*) FROM connector_statuses", 4),
        ("SELECT COUNT(*) FROM connector_auth_states", 3),
    ];

    for (query, expected) in queries {
        let (count,): (i64,) = sqlx::query_as(query).fetch_one(kg.pool()).await.unwrap();
        assert_eq!(count, expected, "{} should return {}", query, expected);
    }
}

#[tokio::test]
async fn migration_051_repoints_legacy_predicate_facts() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");

    // Build a pre-051 database: apply every migration except 051.
    let migrations_dir = dir.path().join("migrations");
    std::fs::create_dir(&migrations_dir).unwrap();
    let migrations_src = concat!(env!("CARGO_MANIFEST_DIR"), "/src/db/migrations");
    for entry in std::fs::read_dir(migrations_src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if name.starts_with("051_") {
            continue;
        }
        std::fs::copy(&path, migrations_dir.join(&name)).unwrap();
    }
    let migrator = sqlx::migrate::Migrator::new(migrations_dir.as_path())
        .await
        .unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=rwc", db_path.display()))
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();

    // Seed legacy facts under the pre-051 vocabulary (based_in, lived_in, is_in
    // are separate canonical rows before migration 051).
    let alice = mimir_knowledge::queries::entity::create_entity(
        &pool,
        "Alice",
        mimir_knowledge::models::entity::EntityType::Person,
        &[],
    )
    .await
    .unwrap()
    .id;
    let london = mimir_knowledge::queries::entity::create_entity(
        &pool,
        "London",
        mimir_knowledge::models::entity::EntityType::Place,
        &[],
    )
    .await
    .unwrap()
    .id;
    for predicate in ["based_in", "lived_in", "is_in"] {
        let (type_id,): (i16,) = sqlx::query_as("SELECT id FROM relationship_types WHERE name = ?")
            .bind(predicate)
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(alice)
        .bind(type_id)
        .bind(london)
        .bind(0.9f32)
        .bind(1i16)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;

    // Upgrade: KnowledgeGraph::init applies the pending 051 migration.
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    // All three legacy facts now point at the consolidated verbs.
    let resides_in = kg
        .get_relationship_type_id("resides_in")
        .await
        .unwrap()
        .unwrap();
    let located_in = kg
        .get_relationship_type_id("located_in")
        .await
        .unwrap()
        .unwrap();
    let (residence_facts,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM facts WHERE subject_id = ? AND relationship_type_id = ?",
    )
    .bind(alice)
    .bind(resides_in)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(
        residence_facts, 2,
        "based_in + lived_in facts repointed to resides_in"
    );
    let (containment_facts,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM facts WHERE subject_id = ? AND relationship_type_id = ?",
    )
    .bind(alice)
    .bind(located_in)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    assert_eq!(containment_facts, 1, "is_in fact repointed to located_in");

    // The old canonical rows are gone and their names resolve as aliases.
    for (alias, canonical) in [
        ("based_in", "resides_in"),
        ("lived_in", "resides_in"),
        ("is_in", "located_in"),
        // Legacy synonyms of lived_in survive the consolidation as aliases.
        ("previously_lived_in", "resides_in"),
        ("former_city", "resides_in"),
    ] {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM relationship_types WHERE name = ?")
                .bind(alias)
                .fetch_one(kg.pool())
                .await
                .unwrap();
        assert_eq!(count, 0, "{alias} must no longer be a canonical row");
        let resolved = kg.resolve_relationship_type_alias(alias).await.unwrap();
        let canonical_id = kg
            .get_relationship_type_id(canonical)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved,
            Some(canonical_id),
            "{alias} must resolve to {canonical}"
        );
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
