//! Roundtrip tests: write each enum variant to the DB via sqlx, read it back,
//! and assert exact equality.

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::*;
use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::preference::{PreferenceCategory, PreferenceSourceType};
use mimir_knowledge::models::source::SourceType;

async fn roundtrip_enum<
    T: sqlx::Type<sqlx::Sqlite>
        + for<'a> sqlx::Decode<'a, sqlx::Sqlite>
        + for<'a> sqlx::Encode<'a, sqlx::Sqlite>
        + std::fmt::Debug
        + PartialEq
        + Copy
        + Send
        + Unpin,
>(
    kg: &KnowledgeGraph,
    query: &'static str,
    values: &[T],
) {
    for &val in values {
        let row: (T,) = sqlx::query_as(query)
            .bind(val)
            .fetch_one(kg.pool())
            .await
            .unwrap();
        assert_eq!(row.0, val);
    }
}

#[tokio::test]
async fn entity_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM entity_types WHERE id = ? LIMIT 1",
        &[
            EntityType::Person,
            EntityType::Place,
            EntityType::Event,
            EntityType::Object,
            EntityType::Concept,
            EntityType::Organization,
            EntityType::Activity,
        ],
    )
    .await;
}

#[tokio::test]
async fn fact_status_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM fact_statuses WHERE id = ? LIMIT 1",
        &[
            FactStatus::Active,
            FactStatus::Inferred,
            FactStatus::Disputed,
            FactStatus::Corrected,
            FactStatus::Superseded,
            FactStatus::Forgotten,
        ],
    )
    .await;
}

#[tokio::test]
async fn source_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM source_types WHERE id = ? LIMIT 1",
        &[
            SourceType::UserEdit,
            SourceType::Connector,
            SourceType::Inference,
            SourceType::Interaction,
            SourceType::Import,
            SourceType::System,
        ],
    )
    .await;
}

#[tokio::test]
async fn relation_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM relation_types WHERE id = ? LIMIT 1",
        &[
            RelationType::InferredFrom,
            RelationType::Corrects,
            RelationType::Supersedes,
        ],
    )
    .await;
}

#[tokio::test]
async fn entity_date_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM entity_date_types WHERE id = ? LIMIT 1",
        &[
            EntityDateType::Birth,
            EntityDateType::Death,
            EntityDateType::Anniversary,
            EntityDateType::Created,
            EntityDateType::Dissolved,
            EntityDateType::Custom,
        ],
    )
    .await;
}

#[tokio::test]
async fn recurrence_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM recurrence_types WHERE id = ? LIMIT 1",
        &[
            RecurrenceType::None,
            RecurrenceType::Daily,
            RecurrenceType::Weekly,
            RecurrenceType::Monthly,
            RecurrenceType::Yearly,
        ],
    )
    .await;
}

#[tokio::test]
async fn location_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM location_types WHERE id = ? LIMIT 1",
        &[
            LocationType::Home,
            LocationType::Work,
            LocationType::Visited,
            LocationType::Origin,
            LocationType::Current,
        ],
    )
    .await;
}

#[tokio::test]
async fn preference_category_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM preference_categories WHERE id = ? LIMIT 1",
        &[
            PreferenceCategory::CalendarBehavior,
            PreferenceCategory::NotificationStyle,
            PreferenceCategory::FoodPreference,
            PreferenceCategory::TravelPreference,
            PreferenceCategory::WorkStyle,
            PreferenceCategory::CommunicationPreference,
            PreferenceCategory::General,
        ],
    )
    .await;
}

#[tokio::test]
async fn preference_source_type_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    roundtrip_enum(
        &kg,
        "SELECT id FROM preference_source_types WHERE id = ? LIMIT 1",
        &[
            PreferenceSourceType::Interaction,
            PreferenceSourceType::Fact,
            PreferenceSourceType::UserEdit,
        ],
    )
    .await;
}
