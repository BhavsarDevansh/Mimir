//! Entity CRUD, alias resolution, type-enum sync, and predicate validation.

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::queries::entity::MatchKind;

mod common;

// ---------------------------------------------------------------------------
// Entity CRUD roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_crud_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Create
    let entity = kg
        .create_entity("Alice", EntityType::Person, &["A. Smith", "Ally"])
        .await
        .unwrap();
    assert_eq!(entity.name, "Alice");
    assert_eq!(entity.entity_type_id, EntityType::Person as i16);

    // Read by ID
    let fetched = kg.get_entity(entity.id).await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.name, "Alice");

    // Update
    let updated = kg
        .update_entity(entity.id, "Alice Smith", EntityType::Person)
        .await
        .unwrap();
    assert_eq!(updated.name, "Alice Smith");

    // Delete (no facts attached)
    kg.delete_entity(entity.id).await.unwrap();
    let gone = kg.get_entity(entity.id).await.unwrap();
    assert!(gone.is_none());
}
// ---------------------------------------------------------------------------
// Alias resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_alias_resolution_exact() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let entity = kg
        .create_entity("Bob", EntityType::Person, &["Bobby", "Robert"])
        .await
        .unwrap();

    // Exact name match
    let results = mimir_knowledge::queries::entity::get_by_name(kg.pool(), "Bob")
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].match_kind, MatchKind::ExactName);
    assert_eq!(results[0].entity.id, entity.id);

    // Exact alias match
    let results = mimir_knowledge::queries::entity::get_by_name(kg.pool(), "Bobby")
        .await
        .unwrap();
    assert!(!results.is_empty());
    let alias_match = results
        .iter()
        .find(|r| r.match_kind == MatchKind::ExactAlias);
    assert!(alias_match.is_some());
    assert_eq!(alias_match.unwrap().entity.id, entity.id);
}

#[tokio::test]
async fn test_alias_outranks_exact_name_when_bare_duplicate_exists() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Canonical entity with alias
    let canonical = kg
        .create_entity("Bob Smith", EntityType::Person, &["Bob"])
        .await
        .unwrap();

    // Bare-name duplicate (accidentally created before alias was wired)
    let duplicate = kg
        .create_entity("Bob", EntityType::Person, &[])
        .await
        .unwrap();

    // Searching for "Bob" should return the canonical entity first because
    // alias matches now outrank exact name matches.
    let results = mimir_knowledge::queries::entity::get_by_name(kg.pool(), "Bob")
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].entity.id, canonical.id);
    assert_eq!(results[0].match_kind, MatchKind::ExactAlias);

    // The duplicate should appear second (exact name match).
    assert_eq!(results[1].entity.id, duplicate.id);
    assert_eq!(results[1].match_kind, MatchKind::ExactName);
}

#[tokio::test]
async fn test_alias_resolution_fuzzy() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    kg.create_entity("Charlotte", EntityType::Person, &[])
        .await
        .unwrap();

    // FTS5 search for exact word (still routed through FTS5 → MatchKind::Fuzzy)
    let results = kg.search_entities("Charlotte", 10).await.unwrap();
    assert!(!results.is_empty());
    let fuzzy = results.iter().find(|r| r.match_kind == MatchKind::Fuzzy);
    assert!(fuzzy.is_some());
}
// ---------------------------------------------------------------------------
// Entity type enum sync (DateTime = 8)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entity_type_enum_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let variants: &[(i16, &str, EntityType)] = &[
        (1, "Person", EntityType::Person),
        (2, "Place", EntityType::Place),
        (3, "Event", EntityType::Event),
        (4, "Object", EntityType::Object),
        (5, "Concept", EntityType::Concept),
        (6, "Organization", EntityType::Organization),
        (7, "Activity", EntityType::Activity),
        (8, "DateTime", EntityType::DateTime),
    ];

    for (expected_id, expected_name, variant) in variants {
        let (db_id, db_name): (i16, String) =
            sqlx::query_as("SELECT id, name FROM entity_types WHERE id = ?")
                .bind(*variant as i16)
                .fetch_one(kg.pool())
                .await
                .unwrap();
        assert_eq!(db_id, *expected_id);
        assert_eq!(db_name, *expected_name);

        // Roundtrip
        let row: (i16,) = sqlx::query_as("SELECT id FROM entity_types WHERE id = ? LIMIT 1")
            .bind(*variant as i16)
            .fetch_one(kg.pool())
            .await
            .unwrap();
        assert_eq!(row.0, *expected_id);
    }

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM entity_types")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(count, variants.len() as i64);
}

// ---------------------------------------------------------------------------
// Predicate validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_predicate_validation() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Valid: Person born_on DateTime
    mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Person,
        common::ensure_relationship_type(&kg, "born_on")
            .await
            .unwrap(),
        EntityType::DateTime,
    )
    .await
    .unwrap();

    // Valid: Organization located_in Place
    mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Organization,
        common::ensure_relationship_type(&kg, "located_in")
            .await
            .unwrap(),
        EntityType::Place,
    )
    .await
    .unwrap();

    // Invalid: Place born_on Person (nonsense combination)
    let result = mimir_knowledge::queries::entity::validate_predicate(
        kg.pool(),
        EntityType::Place,
        common::ensure_relationship_type(&kg, "born_on")
            .await
            .unwrap(),
        EntityType::Person,
    )
    .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Events & reminders (issue #74)
