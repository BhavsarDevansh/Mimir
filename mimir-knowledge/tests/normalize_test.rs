//! Integration tests for the shared `normalize_and_insert` DRY boundary
//! (Phase 3 F4 / issue #181).
//!
//! Both conversational `remember` extraction and connector ingestion funnel
//! through this single deterministic Rust pipeline. These tests exercise the
//! boundary directly with connector provenance — the new code path — and the
//! cross-connector corroboration acceptance criterion.

use chrono::{DateTime, Utc};

use mimir_knowledge::confidence;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{ConnectorType, RecurrenceType};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::{NormalizedFact, Provenance, normalize_and_insert};
use mimir_knowledge::{KnowledgeError, KnowledgeGraph};

/// Fresh KnowledgeGraph in a temp dir.
async fn fresh_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("normalize_test.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone::<Utc>(&Utc)
}

fn rome_event(raw_ref: &str, valid_until: Option<DateTime<Utc>>) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::Connector,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "has_event".to_string(),
        object: "Trip to Rome".to_string(),
        object_is_entity: true,
        object_type: Some(EntityType::Event),
        valid_from: Some(parse_dt("2026-05-03T00:00:00Z")),
        valid_until,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: Some(raw_ref.to_string()),
    }
}

async fn upsert(kg: &KnowledgeGraph, ct: ConnectorType, slug: &str) -> i32 {
    kg.upsert_connector(UpsertConnectorInput {
        connector_type: ct,
        slug: slug.to_string(),
        backend: if ct == ConnectorType::Gmail {
            "imap".to_string()
        } else {
            "caldav".to_string()
        },
        display_name: slug.to_string(),
        config_json: "{}".to_string(),
        status: None,
        auth_state: None,
    })
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn connector_normalized_fact_inserts_with_connector_provenance() {
    let (kg, _dir) = fresh_kg().await;
    let calendar_instance = upsert(&kg, ConnectorType::Calendar, "calendar-1").await;

    let outcome = normalize_and_insert(
        &kg,
        vec![rome_event(
            "cal-evt-123",
            Some(parse_dt("2026-05-07T00:00:00Z")),
        )],
        Provenance::connector(
            calendar_instance,
            ConnectorType::Calendar,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("connector insert should succeed");
    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());

    let inserted = &outcome.inserted[0];
    // Connector reliability score for Calendar, no extraction-method discount.
    assert!(
        (inserted.confidence
            - confidence::initial(SourceType::Connector, Some(ConnectorType::Calendar)))
        .abs()
            < 1e-5,
        "confidence {} should equal Calendar reliability 0.90",
        inserted.confidence
    );

    let sources = kg.get_sources_for_fact(inserted.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::Connector as i16);
    assert_eq!(sources[0].connector_instance_id, Some(calendar_instance));
    assert_eq!(sources[0].raw_reference.as_deref(), Some("cal-evt-123"));
    assert_eq!(
        sources[0].extraction_method_id,
        Some(ExtractionMethod::StructuredParse as i16)
    );
}

#[tokio::test]
async fn cross_connector_corroboration_adds_source_and_boosts_confidence() {
    let (kg, _dir) = fresh_kg().await;
    let calendar_instance = upsert(&kg, ConnectorType::Calendar, "calendar-1").await;
    let gmail_instance = upsert(&kg, ConnectorType::Gmail, "gmail-1").await;

    // Calendar: a "Trip to Rome" event spanning 2026-05-03 .. 2026-05-07.
    let first = normalize_and_insert(
        &kg,
        vec![rome_event(
            "cal-evt-123",
            Some(parse_dt("2026-05-07T00:00:00Z")),
        )],
        Provenance::connector(
            calendar_instance,
            ConnectorType::Calendar,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("calendar insert should succeed");
    assert_eq!(first.inserted.len(), 1);
    let calendar_fact_id = first.inserted[0].id;
    let calendar_confidence = first.inserted[0].confidence;
    assert!(
        (calendar_confidence - 0.90).abs() < 1e-5,
        "Calendar initial confidence should be 0.90, got {calendar_confidence}"
    );

    // Gmail: a flight booking email describing the same trip, overlapping date.
    let second = normalize_and_insert(
        &kg,
        vec![rome_event("gmail-msg-456", None)],
        Provenance::connector(
            gmail_instance,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("gmail corroboration should succeed");

    // Corroboration folds the Gmail fact into the existing Calendar fact:
    // `insert_fact_in_tx` returns the existing fact rather than creating a new
    // row, so the returned "inserted" fact is the *same* Calendar fact id.
    assert!(second.errors.is_empty(), "errors: {:?}", second.errors);
    assert_eq!(second.inserted.len(), 1);
    assert_eq!(
        second.inserted[0].id, calendar_fact_id,
        "corroboration must return the existing fact, not create a new one"
    );

    // Exactly one fact row exists for the Rome claim — no duplicate.
    let devansh = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap()
        .id;
    let facts = kg.get_facts_by_subject(devansh, 100).await.unwrap();
    assert_eq!(facts.len(), 1, "no duplicate fact should be created");
    assert_eq!(facts[0].id, calendar_fact_id);

    // Two independent sources now back the single fact (one per connector).
    let sources = kg.get_sources_for_fact(calendar_fact_id).await.unwrap();
    assert_eq!(
        sources.len(),
        2,
        "both connector sources should be recorded"
    );
    let instance_ids: std::collections::HashSet<Option<i32>> =
        sources.iter().map(|s| s.connector_instance_id).collect();
    assert!(instance_ids.contains(&Some(calendar_instance)));
    assert!(instance_ids.contains(&Some(gmail_instance)));

    // Confidence boosted by +0.05 per independent source, capped at 0.95.
    let corroborated = kg.get_fact(calendar_fact_id).await.unwrap().unwrap();
    assert!(
        (corroborated.confidence - 0.95).abs() < 1e-5,
        "corroborated confidence should be capped at 0.95, got {}",
        corroborated.confidence
    );
}

#[tokio::test]
async fn chat_provenance_inserts_with_interaction_confidence() {
    let (kg, _dir) = fresh_kg().await;

    let fact = NormalizedFact {
        source_type: SourceType::Interaction,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "favourite_colour".to_string(),
        object: "blue".to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: None,
    };

    // Chat casual learning: no connector, LLM extraction method.
    let outcome = normalize_and_insert(
        &kg,
        vec![fact],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .expect("chat insert should succeed");
    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.inserted.len(), 1);

    let inserted = &outcome.inserted[0];
    assert!(
        (inserted.confidence - confidence::initial(SourceType::Interaction, None)).abs() < 1e-5,
        "casual chat confidence should be 0.30, got {}",
        inserted.confidence
    );

    let sources = kg.get_sources_for_fact(inserted.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::Interaction as i16);
    assert!(sources[0].connector_instance_id.is_none());
    assert_eq!(
        sources[0].extraction_method_id,
        Some(ExtractionMethod::LlmExtraction as i16)
    );
}

#[tokio::test]
async fn connector_fact_missing_raw_reference_is_rejected() {
    let (kg, _dir) = fresh_kg().await;
    let calendar_instance = upsert(&kg, ConnectorType::Calendar, "calendar-1").await;

    let mut fact = rome_event("cal-evt-123", None);
    fact.raw_reference = None; // missing — must be rejected by the provenance gate

    let outcome = normalize_and_insert(
        &kg,
        vec![fact],
        Provenance::connector(
            calendar_instance,
            ConnectorType::Calendar,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("batch processing does not abort on per-fact errors");
    assert_eq!(outcome.errors.len(), 1);
    assert!(matches!(
        outcome.errors[0],
        KnowledgeError::Validation(ref msg) if msg.contains("raw_reference")
    ));
    assert!(outcome.inserted.is_empty());
}
