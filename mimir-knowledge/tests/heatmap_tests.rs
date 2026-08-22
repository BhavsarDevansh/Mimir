//! Heatmap aggregation tests: totals, entity/predicate/temporal/confidence
//! distributions, and the forgotten-fact exclusion (issue #69).

use chrono::{DateTime, Utc};
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::{KnowledgeGraph, forget};

async fn create_person(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Person, &[])
        .await
        .unwrap()
        .id
}

async fn create_place(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Place, &[])
        .await
        .unwrap()
        .id
}

fn ts(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&Utc)
}

async fn insert_fact(
    kg: &KnowledgeGraph,
    subject_id: i32,
    relationship_type: &str,
    object_id: Option<i32>,
    object_literal: Option<&str>,
    valid_from: Option<DateTime<Utc>>,
    confidence: Option<f32>,
) -> i32 {
    kg.insert_fact(NewFact {
        subject_id,
        relationship_type: relationship_type.to_string(),
        object_id,
        object_literal: object_literal.map(str::to_string),
        valid_from,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: Some(ExtractionMethod::StructuredParse),
        inferred: false,
        inference_depth: 0,
        confidence,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    })
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn heatmap_aggregates_totals_and_distributions() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let bob = create_person(&kg, "Bob").await;
    let london = create_place(&kg, "London").await;

    insert_fact(
        &kg,
        alice,
        "visited",
        Some(london),
        None,
        Some(ts("2026-01-10T12:00:00Z")),
        Some(1.0),
    )
    .await;
    insert_fact(
        &kg,
        alice,
        "works_as",
        None,
        Some("Engineer"),
        Some(ts("2026-02-10T12:00:00Z")),
        Some(0.85),
    )
    .await;
    insert_fact(
        &kg,
        bob,
        "visited",
        Some(london),
        None,
        Some(ts("2026-02-20T12:00:00Z")),
        Some(0.55),
    )
    .await;
    insert_fact(
        &kg,
        bob,
        "works_as",
        None,
        Some("Chef"),
        Some(ts("2026-03-05T12:00:00Z")),
        Some(0.3),
    )
    .await;

    let heatmap = kg.heatmap().await.unwrap();

    assert_eq!(heatmap.facts, 4);
    assert_eq!(heatmap.entities, 3);
    assert!((heatmap.avg_confidence - 0.675).abs() < 1e-4);

    let top: Vec<(String, i64)> = heatmap
        .top_entities
        .iter()
        .map(|e| (e.name.clone(), e.count))
        .collect();
    // Tie on count is broken by name ascending.
    assert_eq!(top, vec![("Alice".to_string(), 2), ("Bob".to_string(), 2)]);

    let predicates: Vec<(String, i64)> = heatmap
        .predicates
        .iter()
        .map(|p| (p.name.clone(), p.count))
        .collect();
    assert_eq!(
        predicates,
        vec![("visited".to_string(), 2), ("works_as".to_string(), 2)]
    );

    let temporal: Vec<(String, i64)> = heatmap
        .temporal
        .iter()
        .map(|t| (t.period.clone(), t.count))
        .collect();
    assert_eq!(
        temporal,
        vec![
            ("2026-01".to_string(), 1),
            ("2026-02".to_string(), 2),
            ("2026-03".to_string(), 1)
        ]
    );

    let bands: Vec<(&str, i64)> = heatmap
        .confidence_bands
        .iter()
        .map(|b| (b.label.as_str(), b.count))
        .collect();
    assert_eq!(
        bands,
        vec![
            ("explicit (1.0)", 1),
            ("connector (0.7-0.9)", 1),
            ("inference (0.4-0.7)", 1),
            ("casual (<0.4)", 1)
        ]
    );
}

#[tokio::test]
async fn heatmap_excludes_forgotten_facts() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let bob = create_person(&kg, "Bob").await;
    let london = create_place(&kg, "London").await;

    let to_forget = insert_fact(&kg, alice, "visited", Some(london), None, None, Some(1.0)).await;
    insert_fact(&kg, bob, "visited", Some(london), None, None, Some(0.8)).await;
    insert_fact(
        &kg,
        alice,
        "works_as",
        None,
        Some("Engineer"),
        None,
        Some(0.6),
    )
    .await;

    kg.forget_facts(
        forget::ForgetFilters {
            fact_id: Some(to_forget),
            ..Default::default()
        },
        forget::ForgetOptions::default(),
        ChangedBy::User,
    )
    .await
    .unwrap();

    let heatmap = kg.heatmap().await.unwrap();
    assert_eq!(heatmap.facts, 2);
    assert_eq!(heatmap.entities, 3);
    let bands: Vec<i64> = heatmap.confidence_bands.iter().map(|b| b.count).collect();
    assert_eq!(bands, vec![0, 1, 1, 0]);
    let top: Vec<(String, i64)> = heatmap
        .top_entities
        .iter()
        .map(|e| (e.name.clone(), e.count))
        .collect();
    assert_eq!(top, vec![("Alice".to_string(), 1), ("Bob".to_string(), 1)]);
}

#[tokio::test]
async fn heatmap_empty_graph_returns_zeros() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let heatmap = kg.heatmap().await.unwrap();
    assert_eq!(heatmap.facts, 0);
    assert_eq!(heatmap.entities, 0);
    assert_eq!(heatmap.avg_confidence, 0.0);
    assert!(heatmap.top_entities.is_empty());
    assert!(heatmap.predicates.is_empty());
    assert!(heatmap.temporal.is_empty());
    assert!(heatmap.confidence_bands.iter().all(|b| b.count == 0));
}
