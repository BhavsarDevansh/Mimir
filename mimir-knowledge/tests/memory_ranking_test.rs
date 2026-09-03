//! Integration tests for the memory ranking and selection engine (Issue #108).

use chrono::Utc;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::clock::{Clock, MockClock};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn memory_ranking_builds_schema_and_buckets_facts() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let clock = Arc::new(MockClock::new(Utc::now()));
    let kg = KnowledgeGraph::init_with_clock(&db_path, clock.clone())
        .await
        .unwrap();

    // Create the user entity.
    let user = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert an identity fact (category 150 = Identity & Biography).
    let mut identity = NewFact::new(user.id, "works_as");
    identity.object_literal = Some("Software Developer".to_string());
    identity.confidence = Some(0.95);
    identity.source_type = SourceType::UserEdit;
    identity.category_ids = vec![150];
    kg.insert_fact(identity).await.unwrap();

    // Insert a relationship fact (category 420 = Romantic).
    let mut rel = NewFact::new(user.id, "has_partner");
    rel.object_literal = Some("Alice".to_string());
    rel.confidence = Some(0.90);
    rel.source_type = SourceType::UserEdit;
    rel.category_ids = vec![420];
    kg.insert_fact(rel).await.unwrap();

    // Insert a preference fact (category 300 = Food & Diet).
    let mut pref = NewFact::new(user.id, "prefers");
    pref.object_literal = Some("croissants".to_string());
    pref.confidence = Some(0.85);
    pref.source_type = SourceType::UserEdit;
    pref.category_ids = vec![300];
    kg.insert_fact(pref).await.unwrap();

    // Insert an upcoming fact (category 930 = Upcoming Events).
    let mut upcoming = NewFact::new(user.id, "has_appointment");
    upcoming.object_literal = Some("Dentist".to_string());
    upcoming.confidence = Some(0.80);
    upcoming.valid_from = Some(clock.now() + chrono::Duration::days(5));
    upcoming.valid_until = Some(clock.now() + chrono::Duration::days(6));
    upcoming.source_type = SourceType::UserEdit;
    upcoming.category_ids = vec![930];
    kg.insert_fact(upcoming).await.unwrap();

    // Build memory schema.
    let schema = kg.build_memory_schema(user.id, 2500, 0.7).await.unwrap();

    // Identity should have at least the works_as fact.
    assert!(!schema.identity.is_empty(), "expected identity facts");
    assert!(
        schema
            .identity
            .iter()
            .any(|f| f.relationship_type == "works_as")
    );

    // Relationships should have has_partner.
    assert!(
        !schema.relationships.is_empty(),
        "expected relationship facts"
    );
    assert!(
        schema
            .relationships
            .iter()
            .any(|f| f.relationship_type == "has_partner")
    );

    // Preferences should have prefers.
    assert!(!schema.preferences.is_empty(), "expected preference facts");
    assert!(
        schema
            .preferences
            .iter()
            .any(|f| f.relationship_type == "prefers")
    );

    // Upcoming should have has_appointment.
    assert!(!schema.upcoming.is_empty(), "expected upcoming facts");
    assert!(
        schema
            .upcoming
            .iter()
            .any(|f| f.relationship_type == "has_appointment")
    );

    let upcoming_fact = schema
        .upcoming
        .iter()
        .find(|f| f.relationship_type == "has_appointment")
        .unwrap();
    assert_eq!(
        upcoming_fact.valid_from,
        Some(clock.now() + chrono::Duration::days(5))
    );
    assert_eq!(
        upcoming_fact.valid_until,
        Some(clock.now() + chrono::Duration::days(6))
    );
}

#[tokio::test]
async fn memory_ranking_applies_temporal_boost() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let clock = Arc::new(MockClock::new(Utc::now()));
    let kg = KnowledgeGraph::init_with_clock(&db_path, clock.clone())
        .await
        .unwrap();

    let user = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap();

    // Fact A: atemporal, higher confidence.
    let mut atemporal = NewFact::new(user.id, "works_as");
    atemporal.object_literal = Some("Developer".to_string());
    atemporal.confidence = Some(0.95);
    atemporal.source_type = SourceType::UserEdit;
    atemporal.category_ids = vec![150];
    kg.insert_fact(atemporal).await.unwrap();

    // Fact B: upcoming in 2 days, lower confidence.
    let mut upcoming = NewFact::new(user.id, "has_appointment");
    upcoming.object_literal = Some("Dentist".to_string());
    upcoming.confidence = Some(0.80);
    upcoming.valid_from = Some(clock.now() + chrono::Duration::days(2));
    upcoming.source_type = SourceType::UserEdit;
    upcoming.category_ids = vec![930];
    kg.insert_fact(upcoming).await.unwrap();

    let schema = kg.build_memory_schema(user.id, 2500, 0.7).await.unwrap();

    // The upcoming fact should rank higher due to temporal boost (~7.07x).
    let upcoming_score = schema
        .upcoming
        .iter()
        .find(|f| f.relationship_type == "has_appointment")
        .map(|f| f.score)
        .unwrap_or(0.0);

    let identity_score = schema
        .identity
        .iter()
        .find(|f| f.relationship_type == "works_as")
        .map(|f| f.score)
        .unwrap_or(0.0);

    // upcoming (0.80 * ~1.0 * ~7.07) ≈ 5.66 vs identity (0.95 * ~1.0 * 1.0) ≈ 0.95
    assert!(
        upcoming_score > identity_score,
        "expected upcoming score ({}) > identity score ({})",
        upcoming_score,
        identity_score
    );
}

#[tokio::test]
async fn memory_renderer_produces_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let clock = Arc::new(MockClock::new(Utc::now()));
    let kg = KnowledgeGraph::init_with_clock(&db_path, clock.clone())
        .await
        .unwrap();

    let user = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap();

    let mut fact = NewFact::new(user.id, "has_partner");
    fact.object_literal = Some("Alice".to_string());
    fact.confidence = Some(0.90);
    fact.source_type = SourceType::UserEdit;
    fact.category_ids = vec![420];
    kg.insert_fact(fact).await.unwrap();

    let schema = kg.build_memory_schema(user.id, 2500, 0.7).await.unwrap();
    let rendered = kg.render_memory_schema(&schema);

    assert!(
        rendered.contains("Relationships:"),
        "expected header in rendered output: {}",
        rendered
    );
    assert!(
        rendered.contains("is partnered with Alice"),
        "expected readable template: {}",
        rendered
    );
}

#[tokio::test]
async fn system_state_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    assert!(kg.get_condensed_memory().await.unwrap().is_none());

    kg.set_condensed_memory("Devansh works as a developer.")
        .await
        .unwrap();
    let val = kg.get_condensed_memory().await.unwrap();
    assert_eq!(val, Some("Devansh works as a developer.".to_string()));
}

#[tokio::test]
async fn centrality_cache_populates_on_first_build() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let clock = Arc::new(MockClock::new(Utc::now()));
    let kg = KnowledgeGraph::init_with_clock(&db_path, clock.clone())
        .await
        .unwrap();

    let user = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap();

    // Insert multiple facts referencing the same user.
    for i in 0..10 {
        let mut fact = NewFact::new(user.id, "visited");
        fact.object_literal = Some(format!("Place{}", i));
        fact.confidence = Some(0.90);
        fact.source_type = SourceType::UserEdit;
        kg.insert_fact(fact).await.unwrap();
    }

    // Build memory — should populate centrality cache.
    let schema = kg.build_memory_schema(user.id, 2500, 0.7).await.unwrap();
    assert!(!schema.all_facts().is_empty());
}

#[tokio::test]
async fn memory_ranking_assigns_highest_priority_bucket() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");
    let clock = Arc::new(MockClock::new(Utc::now()));
    let kg = KnowledgeGraph::init_with_clock(&db_path, clock.clone())
        .await
        .unwrap();

    let user = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap();

    // Identity beats upcoming (150 Identity + 930 Upcoming).
    let mut identity = NewFact::new(user.id, "works_as");
    identity.object_literal = Some("Developer".to_string());
    identity.confidence = Some(0.95);
    identity.source_type = SourceType::UserEdit;
    identity.category_ids = vec![150, 930];
    kg.insert_fact(identity).await.unwrap();

    // Upcoming beats relationships (930 Upcoming + 420 Romantic).
    let mut upcoming = NewFact::new(user.id, "has_appointment");
    upcoming.object_literal = Some("Dentist".to_string());
    upcoming.confidence = Some(0.90);
    upcoming.valid_from = Some(clock.now() + chrono::Duration::days(5));
    upcoming.source_type = SourceType::UserEdit;
    upcoming.category_ids = vec![930, 420];
    kg.insert_fact(upcoming).await.unwrap();

    // Relationships beats preferences (420 Romantic + 300 Health).
    let mut rel = NewFact::new(user.id, "has_partner");
    rel.object_literal = Some("Alice".to_string());
    rel.confidence = Some(0.90);
    rel.source_type = SourceType::UserEdit;
    rel.category_ids = vec![420, 300];
    kg.insert_fact(rel).await.unwrap();

    // Preferences beat general (300 Health + 500 Work).
    let mut pref = NewFact::new(user.id, "prefers");
    pref.object_literal = Some("mild".to_string());
    pref.confidence = Some(0.85);
    pref.source_type = SourceType::UserEdit;
    pref.category_ids = vec![300, 500];
    kg.insert_fact(pref).await.unwrap();

    // General-only fact. This test intentionally uses a taxonomy-external
    // type to verify bucketing without a deterministic domain fallback.
    let general_type_id = kg.ensure_relationship_type("test_general").await.unwrap();
    let mut general = NewFact::new(user.id, "test_general");
    general.object_literal = Some("hiking".to_string());
    general.source_type = SourceType::UserEdit;
    general.category_ids = Vec::new();
    mimir_knowledge::queries::fact::insert_fact(
        kg.pool(),
        &general,
        general_type_id,
        0.85,
        Utc::now(),
    )
    .await
    .unwrap();

    let schema = kg.build_memory_schema(user.id, 2500, 0.7).await.unwrap();

    let in_bucket = |bucket: &[mimir_knowledge::models::memory::RankedFact], predicate: &str| {
        bucket.iter().any(|f| f.relationship_type == predicate)
    };

    assert!(
        in_bucket(&schema.identity, "works_as"),
        "multi-category fact must land in Identity"
    );
    assert!(
        in_bucket(&schema.upcoming, "has_appointment"),
        "multi-category fact must land in Upcoming"
    );
    assert!(
        in_bucket(&schema.relationships, "has_partner"),
        "multi-category fact must land in Relationships"
    );
    assert!(
        in_bucket(&schema.preferences, "prefers"),
        "multi-category fact must land in Preferences"
    );
    assert!(
        in_bucket(&schema.general, "test_general"),
        "uncategorised-domain fact must land in General"
    );

    assert!(!in_bucket(&schema.identity, "has_appointment"));
    assert!(!in_bucket(&schema.upcoming, "works_as"));
}
