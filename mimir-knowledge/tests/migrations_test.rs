//! Tests that all migrations run cleanly and produce the expected schema.

use mimir_knowledge::KnowledgeGraph;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;

/// Build a pre-051 database: apply every migration below 051 and return the
/// open pool so the caller can seed legacy data before upgrading.
async fn build_pre_051_pool(dir: &tempfile::TempDir) -> SqlitePool {
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let migrations_dir = dir.path().join("migrations");
    std::fs::create_dir(&migrations_dir).unwrap();
    let migrations_src = concat!(env!("CARGO_MANIFEST_DIR"), "/src/db/migrations");
    for entry in std::fs::read_dir(migrations_src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        // Everything from 051 onwards is the upgrade under test.
        let version: u32 = name
            .split('_')
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or(u32::MAX);
        if version >= 51 {
            continue;
        }
        std::fs::copy(&path, migrations_dir.join(&name)).unwrap();
    }
    let migrator = sqlx::migrate::Migrator::new(migrations_dir.as_path())
        .await
        .unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=rwc", db_path.display()))
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();
    pool
}

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
    assert!(names.contains(&"memory_buckets".to_string()));
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
        ("SELECT COUNT(*) FROM relationship_types", 62),
        ("SELECT COUNT(*) FROM memory_buckets", 5),
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
    let pool = build_pre_051_pool(&dir).await;

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
async fn migration_051_merges_pre_existing_resides_in_row() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");

    // Pre-051 database, then simulate a pre-036-era auto-created `resides_in`
    // row with a fact alongside the canonical `based_in` row migration 036
    // seeds. Pre-036 the extractor emitted `resides_in` before the alias table
    // existed, so such a row owns the `resides_in` alias (036's INSERT OR
    // IGNORE lost the PK conflict) and migration 050 preserves it because it
    // has facts.
    let pool = build_pre_051_pool(&dir).await;
    let based_in_id: i16 =
        sqlx::query_scalar("SELECT id FROM relationship_types WHERE name = 'based_in'")
            .fetch_one(&pool)
            .await
            .unwrap();
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
    let (auto_id,): (i16,) = sqlx::query_as(
        "INSERT INTO relationship_types (name, description) VALUES ('resides_in', 'Auto-created relationship_type: resides_in') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // The pre-036 self-alias owns the `resides_in` name.
    sqlx::query(
        "INSERT OR REPLACE INTO relationship_type_aliases (alias, relationship_type_id) VALUES ('resides_in', ?)",
    )
    .bind(auto_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO facts (subject_id, relationship_type_id, object_id, confidence, fact_status_id) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(alice)
    .bind(auto_id)
    .bind(london)
    .bind(0.9f32)
    .bind(1i16)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // Upgrade: the merge must fold the auto-created row into based_in before
    // the rename, otherwise the UNIQUE name constraint would fail migration 051.
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let (resides_rows,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM relationship_types WHERE name = 'resides_in'")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(
        resides_rows, 1,
        "exactly one resides_in canonical row survives"
    );
    let (resides_id,): (i16,) =
        sqlx::query_as("SELECT id FROM relationship_types WHERE name = 'resides_in'")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(resides_id, based_in_id, "rename keeps based_in's id");
    let (facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE relationship_type_id = ?")
            .bind(resides_id)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(
        facts, 1,
        "auto-created row's fact follows the canonical verb"
    );
    assert_eq!(
        kg.resolve_relationship_type_alias("resides_in")
            .await
            .unwrap(),
        Some(resides_id),
        "resides_in alias resolves to the surviving row"
    );
    assert_eq!(
        kg.resolve_relationship_type_alias("based_in")
            .await
            .unwrap(),
        Some(resides_id),
        "based_in alias resolves to the surviving row"
    );
    let (description,): (String,) =
        sqlx::query_as("SELECT description FROM relationship_types WHERE name = 'resides_in'")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert!(
        !description.starts_with("Auto-created"),
        "resides_in carries the canonical description: {description}"
    );
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
async fn connector_type_lookup_seed_matches_enum() {
    // Issue #400: the persisted connector type for IMAP mail is the generic
    // `email` type (id 1, renamed from "Gmail" by migration 054) so an
    // Outlook/Yahoo/Proton connector is never labelled "gmail". The lookup
    // seed must stay in lockstep with the `ConnectorType` enum.
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let rows: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, name FROM connector_types ORDER BY id")
            .fetch_all(kg.pool())
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![
            (1, "Email".to_string()),
            (2, "Calendar".to_string()),
            (3, "Photos".to_string()),
            (4, "LinkedIn".to_string()),
        ],
        "connector_types lookup seed drifted from the ConnectorType enum"
    );
    let (score,): (f64,) =
        sqlx::query_as("SELECT score FROM connector_reliability WHERE connector_type_id = 1")
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(
        score, 0.85,
        "email reliability score must survive the rename"
    );
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

#[tokio::test]
async fn category_memory_bucket_seed_pins_taxonomy() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    let rows: Vec<(i32, Option<String>)> = sqlx::query_as(
        "SELECT c.id, b.name \
         FROM categories c \
         LEFT JOIN memory_buckets b ON b.id = c.memory_bucket_id \
         ORDER BY c.id",
    )
    .fetch_all(kg.pool())
    .await
    .unwrap();

    assert_eq!(rows.len(), 92, "category taxonomy seed drifted");
    for (id, bucket) in rows {
        let expected = expected_memory_bucket_name(id);
        assert_eq!(
            bucket.as_deref(),
            Some(expected),
            "category {id} has the wrong memory bucket"
        );
    }
}

/// Expected memory bucket per seeded category id, mirroring the taxonomy seed
/// (migration 031) and the bucket backfill (migration 052): identity 100-199,
/// upcoming 900-999, relationships 400-499 (including 460/480), preferences
/// 300-399 plus the preference-ish outliers (570, 670, 680, 690, 830, 870),
/// everything else general.
fn expected_memory_bucket_name(id: i32) -> &'static str {
    match id {
        100..=199 => "Identity",
        900..=999 => "Upcoming",
        400..=499 => "Relationships",
        300..=399 | 570 | 670 | 680 | 690 | 830 | 870 => "Preferences",
        _ => "General",
    }
}
