//! Bidirectional sync tests: every enum variant maps to a DB row and vice versa.

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::*;
use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::preference::{PreferenceCategory, PreferenceSourceType};
use mimir_knowledge::models::source::SourceType;

async fn assert_enum_db_sync<T: std::fmt::Debug + PartialEq>(
    kg: &KnowledgeGraph,
    select_one_query: &'static str,
    select_all_query: &'static str,
    count_query: &'static str,
    variants: &[(i16, &str, T)],
) {
    // 1. Every known enum variant has a DB row with matching (id, name).
    for &(expected_id, expected_name, _) in variants {
        let (db_id, db_name): (i16, String) = sqlx::query_as(select_one_query)
            .bind(expected_id)
            .fetch_one(kg.pool())
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "DB row missing for id={} name={}: {}",
                    expected_id, expected_name, e
                )
            });
        assert_eq!(db_id, expected_id);
        assert_eq!(db_name, expected_name);
    }

    // 2. Every DB row matches a known enum variant.
    let rows: Vec<(i16, String)> = sqlx::query_as(select_all_query)
        .fetch_all(kg.pool())
        .await
        .unwrap();

    for (db_id, db_name) in rows {
        let found = variants
            .iter()
            .any(|(id, name, _)| *id == db_id && *name == db_name);
        assert!(
            found,
            "DB row ({}, {}) has no matching enum variant",
            db_id, db_name
        );
    }

    // 3. Row count matches variant count.
    let (count,): (i64,) = sqlx::query_as(count_query)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert_eq!(variants.len() as i64, count, "row count mismatch");
}

#[tokio::test]
async fn entity_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM entity_types WHERE id = ?",
        "SELECT id, name FROM entity_types",
        "SELECT COUNT(*) FROM entity_types",
        &[
            (1, "Person", EntityType::Person),
            (2, "Place", EntityType::Place),
            (3, "Event", EntityType::Event),
            (4, "Object", EntityType::Object),
            (5, "Concept", EntityType::Concept),
            (6, "Organization", EntityType::Organization),
            (7, "Activity", EntityType::Activity),
            (8, "DateTime", EntityType::DateTime),
        ],
    )
    .await;
}

#[tokio::test]
async fn entity_date_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM entity_date_types WHERE id = ?",
        "SELECT id, name FROM entity_date_types",
        "SELECT COUNT(*) FROM entity_date_types",
        &[
            (1, "Birth", EntityDateType::Birth),
            (2, "Death", EntityDateType::Death),
            (3, "Anniversary", EntityDateType::Anniversary),
            (4, "Created", EntityDateType::Created),
            (5, "Dissolved", EntityDateType::Dissolved),
            (6, "Custom", EntityDateType::Custom),
        ],
    )
    .await;
}

#[tokio::test]
async fn recurrence_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM recurrence_types WHERE id = ?",
        "SELECT id, name FROM recurrence_types",
        "SELECT COUNT(*) FROM recurrence_types",
        &[
            (1, "None", RecurrenceType::None),
            (2, "Daily", RecurrenceType::Daily),
            (3, "Weekly", RecurrenceType::Weekly),
            (4, "Monthly", RecurrenceType::Monthly),
            (5, "Yearly", RecurrenceType::Yearly),
        ],
    )
    .await;
}

#[tokio::test]
async fn location_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM location_types WHERE id = ?",
        "SELECT id, name FROM location_types",
        "SELECT COUNT(*) FROM location_types",
        &[
            (1, "Home", LocationType::Home),
            (2, "Work", LocationType::Work),
            (3, "Visited", LocationType::Visited),
            (4, "Origin", LocationType::Origin),
            (5, "Current", LocationType::Current),
        ],
    )
    .await;
}

#[tokio::test]
async fn fact_statuses_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM fact_statuses WHERE id = ?",
        "SELECT id, name FROM fact_statuses",
        "SELECT COUNT(*) FROM fact_statuses",
        &[
            (1, "Active", FactStatus::Active),
            (2, "Inferred", FactStatus::Inferred),
            (3, "Disputed", FactStatus::Disputed),
            (4, "Corrected", FactStatus::Corrected),
            (5, "Superseded", FactStatus::Superseded),
            (6, "Forgotten", FactStatus::Forgotten),
        ],
    )
    .await;
}

#[tokio::test]
async fn relation_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM relation_types WHERE id = ?",
        "SELECT id, name FROM relation_types",
        "SELECT COUNT(*) FROM relation_types",
        &[
            (1, "InferredFrom", RelationType::InferredFrom),
            (2, "Corrects", RelationType::Corrects),
            (3, "Supersedes", RelationType::Supersedes),
        ],
    )
    .await;
}

#[tokio::test]
async fn source_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM source_types WHERE id = ?",
        "SELECT id, name FROM source_types",
        "SELECT COUNT(*) FROM source_types",
        &[
            (1, "Email", SourceType::Email),
            (2, "Calendar", SourceType::Calendar),
            (3, "Photo", SourceType::Photo),
            (4, "Message", SourceType::Message),
            (5, "Inference", SourceType::Inference),
            (6, "UserEdit", SourceType::UserEdit),
            (7, "Connector", SourceType::Connector),
            (8, "CasualMention", SourceType::CasualMention),
            (9, "Import", SourceType::Import),
            (10, "System", SourceType::System),
        ],
    )
    .await;
}

#[tokio::test]
async fn preference_categories_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM preference_categories WHERE id = ?",
        "SELECT id, name FROM preference_categories",
        "SELECT COUNT(*) FROM preference_categories",
        &[
            (
                1,
                "NotificationStyle",
                PreferenceCategory::NotificationStyle,
            ),
            (2, "CalendarAutoAdd", PreferenceCategory::CalendarAutoAdd),
            (3, "ProactivityLevel", PreferenceCategory::ProactivityLevel),
            (
                4,
                "CommunicationTone",
                PreferenceCategory::CommunicationTone,
            ),
            (5, "Privacy", PreferenceCategory::Privacy),
        ],
    )
    .await;
}

#[tokio::test]
async fn preference_source_types_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM preference_source_types WHERE id = ?",
        "SELECT id, name FROM preference_source_types",
        "SELECT COUNT(*) FROM preference_source_types",
        &[
            (1, "Explicit", PreferenceSourceType::Explicit),
            (2, "Inferred", PreferenceSourceType::Inferred),
            (3, "Corrected", PreferenceSourceType::Corrected),
        ],
    )
    .await;
}

#[tokio::test]
async fn predicates_sync() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    assert_enum_db_sync(
        &kg,
        "SELECT id, name FROM predicates WHERE id = ?",
        "SELECT id, name FROM predicates",
        "SELECT COUNT(*) FROM predicates",
        &[
            (1, "is_in", Predicate::IsIn),
            (2, "visited", Predicate::Visited),
            (3, "owns", Predicate::Owns),
            (4, "works_as", Predicate::WorksAs),
            (5, "has_partner", Predicate::HasPartner),
            (6, "has_parent", Predicate::HasParent),
            (7, "born_on", Predicate::BornOn),
            (8, "died_on", Predicate::DiedOn),
            (9, "located_in", Predicate::LocatedIn),
            (10, "created_on", Predicate::CreatedOn),
        ],
    )
    .await;
}
