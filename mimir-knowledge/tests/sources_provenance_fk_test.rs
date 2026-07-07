//! Sources provenance FK migration tests (issue #180 / Phase 3 F3).
//!
//! Covers the new behaviour introduced by migrating
//! `sources.connector_id TEXT` -> `connector_instance_id INTEGER REFERENCES
//! connectors(id)`: the validation gate enforces that a connector fact's
//! denormalised `connector_type` matches its registered instance (or derives
//! it when omitted), and per-connector item counts are derivable from the
//! `sources` table.

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::ConnectorType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

async fn person(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Person, &[])
        .await
        .unwrap()
        .id
}

async fn place(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Place, &[])
        .await
        .unwrap()
        .id
}

fn gmail_input(slug: &str) -> UpsertConnectorInput {
    UpsertConnectorInput {
        connector_type: ConnectorType::Gmail,
        slug: slug.to_string(),
        backend: "imap".to_string(),
        display_name: "Personal Gmail".to_string(),
        config_json: "{}".to_string(),
        status: None,
        auth_state: None,
    }
}

#[tokio::test]
async fn connector_fact_round_trips_integer_instance_id() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let instance = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_instance_id: Some(instance.id),
            connector_type: Some(ConnectorType::Gmail),
            raw_reference: Some("msg-1".to_string()),
            extraction_method: Some(ExtractionMethod::StructuredParse),
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    let sources = kg.get_sources_for_fact(fact.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].connector_instance_id, Some(instance.id));
    // The denormalised connector_type_id is retained (Gmail = 1).
    assert_eq!(
        sources[0].connector_type_id,
        Some(ConnectorType::Gmail as i16)
    );
}

#[tokio::test]
async fn item_count_derivable_per_connector_instance() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let gmail = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();
    let calendar = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Calendar,
            slug: "cal-1".to_string(),
            backend: "caldav".to_string(),
            display_name: "Calendar".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    let mk = |instance_id: i32, raw: &str| NewFact {
        subject_id: alice,
        relationship_type: "is_in".to_string(),
        object_id: Some(london),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_id),
        connector_type: None, // derived by the gate from the instance
        raw_reference: Some(raw.to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };

    kg.insert_fact(mk(gmail.id, "g-1")).await.unwrap();
    kg.insert_fact(mk(gmail.id, "g-2")).await.unwrap();
    kg.insert_fact(mk(calendar.id, "c-1")).await.unwrap();

    let gmail_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?")
            .bind(gmail.id)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    let calendar_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?")
            .bind(calendar.id)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(gmail_count, 2);
    assert_eq!(calendar_count, 1);
}

#[tokio::test]
async fn gate_rejects_connector_type_mismatch() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    // Register a Gmail instance, but claim it is a Calendar fact.
    let gmail = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    let result = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_instance_id: Some(gmail.id),
            connector_type: Some(ConnectorType::Calendar),
            raw_reference: Some("msg-1".to_string()),
            extraction_method: Some(ExtractionMethod::StructuredParse),
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await;
    assert!(
        result.is_err(),
        "type mismatch between instance and denormalised connector_type must be rejected"
    );
}

#[tokio::test]
async fn gate_rejects_missing_raw_reference_or_extraction_method() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let gmail = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    // Missing raw_reference.
    let missing_ref = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_instance_id: Some(gmail.id),
            connector_type: Some(ConnectorType::Gmail),
            raw_reference: None,
            extraction_method: Some(ExtractionMethod::StructuredParse),
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await;
    assert!(
        missing_ref.is_err(),
        "missing raw_reference must be rejected"
    );

    // Missing extraction_method.
    let missing_method = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_instance_id: Some(gmail.id),
            connector_type: Some(ConnectorType::Gmail),
            raw_reference: Some("msg-1".to_string()),
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await;
    assert!(
        missing_method.is_err(),
        "missing extraction_method must be rejected"
    );
}

#[tokio::test]
async fn gate_rejects_unknown_connector_instance() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;

    let result = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_instance_id: Some(999_999), // no such instance
            connector_type: Some(ConnectorType::Gmail),
            raw_reference: Some("msg-1".to_string()),
            extraction_method: Some(ExtractionMethod::StructuredParse),
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await;
    assert!(
        result.is_err(),
        "an unregistered connector_instance_id must be rejected (FK / not found)"
    );
}

#[tokio::test]
async fn gate_derives_connector_type_from_instance_when_omitted() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    // Adjust Calendar reliability away from its default to prove the derived
    // type drives the confidence lookup.
    kg.adjust_connector_reliability(ConnectorType::Calendar, -0.05)
        .await
        .unwrap();
    let calendar = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Calendar,
            slug: "cal-1".to_string(),
            backend: "caldav".to_string(),
            display_name: "Calendar".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_instance_id: Some(calendar.id),
            connector_type: None, // gate derives Calendar from the instance
            raw_reference: Some("evt-1".to_string()),
            extraction_method: Some(ExtractionMethod::StructuredParse),
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Calendar default is 0.90; after -0.05 the reliability is 0.85.
    assert!(
        (fact.confidence - 0.85).abs() < 1e-4,
        "expected derived Calendar confidence 0.85, got {}",
        fact.confidence
    );
    let sources = kg.get_sources_for_fact(fact.id).await.unwrap();
    assert_eq!(
        sources[0].connector_type_id,
        Some(ConnectorType::Calendar as i16)
    );
}
