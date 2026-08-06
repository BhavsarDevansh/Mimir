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
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};
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

/// Shared `NewFact` builder for connector-provenance tests: centralises the
/// common fields and exposes only the values that vary per test
/// (`connector_instance_id`, `connector_type`, `raw_reference`,
/// `extraction_method`). Mirrors the `connector_new_fact` helper in
/// `fact_management_test.rs` and the `gmail_input` pattern above.
#[allow(clippy::too_many_arguments)]
fn connector_fact(
    subject_id: i32,
    object_id: Option<i32>,
    connector_instance_id: Option<i32>,
    connector_type: Option<ConnectorType>,
    raw_reference: Option<&str>,
    extraction_method: Option<ExtractionMethod>,
) -> NewFact {
    NewFact {
        subject_id,
        relationship_type: "is_in".to_string(),
        object_id,
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::Connector,
        connector_instance_id,
        connector_type,
        raw_reference: raw_reference.map(str::to_string),
        extraction_method,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    }
}

#[tokio::test]
async fn connector_fact_round_trips_integer_instance_id() {
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let instance = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    let fact = kg
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(instance.id),
            Some(ConnectorType::Gmail),
            Some("msg-1"),
            Some(ExtractionMethod::StructuredParse),
        ))
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

    let mk = |instance_id: i32, raw: &str| {
        connector_fact(
            alice,
            Some(london),
            Some(instance_id),
            None, // derived by the gate from the instance
            Some(raw),
            Some(ExtractionMethod::StructuredParse),
        )
    };

    kg.insert_fact(mk(gmail.id, "g-1")).await.unwrap();
    kg.insert_fact(mk(gmail.id, "g-2")).await.unwrap();
    kg.insert_fact(mk(calendar.id, "c-1")).await.unwrap();

    let gmail_count = kg.count_sources_for_connector(gmail.id).await.unwrap();
    let calendar_count = kg.count_sources_for_connector(calendar.id).await.unwrap();
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
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(gmail.id),
            Some(ConnectorType::Calendar),
            Some("msg-1"),
            Some(ExtractionMethod::StructuredParse),
        ))
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
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(gmail.id),
            Some(ConnectorType::Gmail),
            None,
            Some(ExtractionMethod::StructuredParse),
        ))
        .await;
    assert!(
        missing_ref.is_err(),
        "missing raw_reference must be rejected"
    );

    // Missing extraction_method.
    let missing_method = kg
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(gmail.id),
            Some(ConnectorType::Gmail),
            Some("msg-1"),
            None,
        ))
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
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(999_999), // no such instance
            Some(ConnectorType::Gmail),
            Some("msg-1"),
            Some(ExtractionMethod::StructuredParse),
        ))
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
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(calendar.id),
            None, // gate derives Calendar from the instance
            Some("evt-1"),
            Some(ExtractionMethod::StructuredParse),
        ))
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

#[tokio::test]
async fn gate_rejects_instance_with_type_outside_connector_type_enum() {
    // Regression for a latent panic: if a connectors row references a
    // connector_types id that is not a ConnectorType enum variant, the gate
    // must surface a validation error rather than panicking on the derived
    // connector_type.
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;

    // Seed a connector_types row outside the ConnectorType enum, then a
    // connectors instance pointing at it (bypassing upsert_connector, which
    // only accepts known ConnectorType values).
    sqlx::query("INSERT INTO connector_types (id, name) VALUES (?, ?)")
        .bind(99_i16)
        .bind("Experimental")
        .execute(kg.pool())
        .await
        .unwrap();
    let instance_id: i32 = sqlx::query_scalar(
        "INSERT INTO connectors \
         (connector_type_id, slug, backend, display_name, config_json, status_id, auth_state_id, \
         created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP) \
         RETURNING id",
    )
    .bind(99_i16)
    .bind("experimental")
    .bind("test")
    .bind("Experimental")
    .bind("{}")
    // Derive status/auth-state ids from the enum variants upsert_connector
    // defaults to (Setup / Unauthenticated) rather than hardcoding ordinals,
    // so the test stays correct if the enum repr ever changes.
    .bind(ConnectorStatus::Setup as i16)
    .bind(ConnectorAuthState::Unauthenticated as i16)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    let result = kg
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(instance_id),
            None, // gate must derive and reject, not panic
            Some("x"),
            Some(ExtractionMethod::StructuredParse),
        ))
        .await;
    assert!(
        result.is_err(),
        "an instance whose type is outside the ConnectorType enum must be rejected, not panicked on"
    );
}

#[tokio::test]
async fn gate_still_requires_raw_reference_when_confidence_is_explicit() {
    // Regression for the confidence fast-path bypass: an explicit confidence
    // must not skip connector provenance validation. A connector fact missing
    // raw_reference is rejected even when confidence is supplied.
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let gmail = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    let mut fact = connector_fact(
        alice,
        Some(london),
        Some(gmail.id),
        Some(ConnectorType::Gmail),
        None, // missing raw_reference
        Some(ExtractionMethod::StructuredParse),
    );
    fact.confidence = Some(0.99);

    let result = kg.insert_fact(fact).await;
    assert!(
        result.is_err(),
        "explicit confidence must not bypass the raw_reference provenance check"
    );
}

#[tokio::test]
async fn gate_still_rejects_type_mismatch_when_confidence_is_explicit() {
    // Regression for the confidence fast-path bypass: an explicit confidence
    // must not skip the connector_type consistency check. A type mismatch
    // between the instance and the denormalised connector_type is rejected
    // even when confidence is supplied.
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let gmail = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    let mut fact = connector_fact(
        alice,
        Some(london),
        Some(gmail.id),
        Some(ConnectorType::Calendar), // mismatched: instance is Gmail
        Some("msg-1"),
        Some(ExtractionMethod::StructuredParse),
    );
    fact.confidence = Some(0.99);

    let result = kg.insert_fact(fact).await;
    assert!(
        result.is_err(),
        "explicit confidence must not bypass the connector_type consistency check"
    );
}

#[tokio::test]
async fn delete_connector_detaches_provenance_preserving_facts() {
    // A1 / #202: deleting a connector instance must null its sources FK so the
    // facts survive with degraded provenance (the full forget cascade is A2).
    let (kg, _dir) = init_kg().await;
    let alice = person(&kg, "Alice").await;
    let london = place(&kg, "London").await;
    let gmail = kg.upsert_connector(gmail_input("gmail-1")).await.unwrap();

    let fact = kg
        .insert_fact(connector_fact(
            alice,
            Some(london),
            Some(gmail.id),
            Some(ConnectorType::Gmail),
            Some("msg-1"),
            Some(ExtractionMethod::StructuredParse),
        ))
        .await
        .unwrap();
    assert_eq!(kg.count_sources_for_connector(gmail.id).await.unwrap(), 1);

    kg.delete_connector(gmail.id).await.unwrap();

    // The fact and its source row survive; the instance reference is nulled.
    let sources = kg.get_sources_for_fact(fact.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].connector_instance_id, None);
    // The denormalised connector kind is retained for post-deletion queries.
    assert_eq!(
        sources[0].connector_type_id,
        Some(ConnectorType::Gmail as i16)
    );
    assert_eq!(kg.count_sources_for_connector(gmail.id).await.unwrap(), 0);
}
