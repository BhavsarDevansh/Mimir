//! Integration tests for the entity-locations write path (Phase 3 S3 / #193).
//!
//! A `NormalizedFact` carrying a `NormalizedLocation` overlay is turned into an
//! `entity_locations` row for the resolved subject entity, geocoding the missing
//! half via the injected `Geocoder` and superseding prior open-ended locations
//! of the same type on a move.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use mimir_core::geocoder::{GeocodeResult, MockGeocoder};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{ConnectorType, LocationType};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::models::{self};
use mimir_knowledge::normalize::{
    NormalizedFact, NormalizedLocation, Provenance, normalize_and_insert,
};

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone::<Utc>(&Utc)
}

async fn fresh_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("locations.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn london_result() -> GeocodeResult {
    GeocodeResult {
        latitude: 51.5074,
        longitude: -0.1278,
        display_name: "London, Greater London, England, United Kingdom".to_string(),
        country: Some("United Kingdom".to_string()),
        country_code: Some("gb".to_string()),
        alternative_names: vec![],
    }
}

/// A "where" fact: "Devansh lives at <object>" with a Home overlay.
fn home_fact(
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    valid_from: Option<DateTime<Utc>>,
) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::UserEdit,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "lives_at".to_string(),
        object: "10 Downing St".to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: models::enums::RecurrenceType::None,
        requires_user_action: false,
        raw_reference: None,
        location: Some(NormalizedLocation {
            location_type: LocationType::Home,
            address: address.map(str::to_string),
            latitude,
            longitude,
            timezone: Some("Europe/London".to_string()),
        }),
    }
}

async fn subject_locations(
    kg: &KnowledgeGraph,
    fact_id_subject: i32,
) -> Vec<models::entity_location::EntityLocation> {
    kg.get_locations(fact_id_subject).await.unwrap()
}

#[tokio::test]
async fn address_only_is_forward_geocoded_and_persisted() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_forward(Ok(Some(london_result()))),
    ));

    let outcome = normalize_and_insert(
        &kg,
        vec![home_fact(Some("10 Downing St, London"), None, None, None)],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.errors.is_empty());

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 1);
    let loc = &locs[0];
    assert_eq!(loc.location_type_id, LocationType::Home as i16);
    assert_eq!(loc.address.as_deref(), Some("10 Downing St, London"));
    assert!((loc.latitude.unwrap() - 51.5074).abs() < 1e-6);
    assert!((loc.longitude.unwrap() - -0.1278).abs() < 1e-6);
    assert_eq!(loc.timezone.as_deref(), Some("Europe/London"));
    assert_eq!(loc.source_fact_id, Some(outcome.inserted[0].id));
}

#[tokio::test]
async fn coords_only_is_reverse_geocoded_to_address() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_reverse(Ok(Some(london_result()))),
    ));

    let outcome = normalize_and_insert(
        &kg,
        vec![home_fact(None, Some(51.5074), Some(-0.1278), None)],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 1);
    let loc = &locs[0];
    assert_eq!(
        loc.address.as_deref(),
        Some(london_result().display_name.as_str())
    );
    assert!((loc.latitude.unwrap() - 51.5074).abs() < 1e-6);
}

#[tokio::test]
async fn both_present_stored_without_geocoding() {
    let (kg, _dir) = fresh_kg().await;
    // No geocoder injected; both halves present so none is needed.
    let outcome = normalize_and_insert(
        &kg,
        vec![home_fact(
            Some("10 Downing St"),
            Some(51.0),
            Some(-0.1),
            None,
        )],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 1);
    let loc = &locs[0];
    assert_eq!(loc.address.as_deref(), Some("10 Downing St"));
    assert!((loc.latitude.unwrap() - 51.0).abs() < 1e-6);
    assert!((loc.longitude.unwrap() - -0.1).abs() < 1e-6);
}

#[tokio::test]
async fn no_geocoder_stores_address_only() {
    let (kg, _dir) = fresh_kg().await;
    let outcome = normalize_and_insert(
        &kg,
        vec![home_fact(Some("10 Downing St"), None, None, None)],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].address.as_deref(), Some("10 Downing St"));
    assert!(locs[0].latitude.is_none());
    assert!(locs[0].longitude.is_none());
}

#[tokio::test]
async fn geocoder_error_is_tolerated() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(MockGeocoder::new().with_forward(Err(
        mimir_core::geocoder::GeocodeError::Network("boom".to_string()),
    ))));

    let outcome = normalize_and_insert(
        &kg,
        vec![home_fact(Some("10 Downing St"), None, None, None)],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.errors.is_empty());

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].address.as_deref(), Some("10 Downing St"));
    assert!(locs[0].latitude.is_none());
}

#[tokio::test]
async fn move_supersedes_prior_open_location() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_forward(Ok(Some(london_result()))),
    ));

    let from_2020 = parse_dt("2020-01-01T00:00:00Z");
    let from_2023 = parse_dt("2023-06-01T00:00:00Z");

    normalize_and_insert(
        &kg,
        vec![home_fact(Some("Old Road"), None, None, Some(from_2020))],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    let second = normalize_and_insert(
        &kg,
        vec![home_fact(Some("New Road"), None, None, Some(from_2023))],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    let locs = subject_locations(&kg, second.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 2, "both rows should coexist");
    let old = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("Old Road"))
        .unwrap();
    let new = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("New Road"))
        .unwrap();
    assert_eq!(old.valid_from, Some(from_2020));
    assert_eq!(
        old.valid_until,
        Some(from_2023),
        "prior open location closed at the move date"
    );
    assert_eq!(new.valid_from, Some(from_2023));
    assert!(new.valid_until.is_none(), "new location is open-ended");
}

#[tokio::test]
async fn connector_location_overlay_persists() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_forward(Ok(Some(london_result()))),
    ));

    let instance = kg
        .upsert_connector(models::connector::UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gmail-1".to_string(),
            backend: "imap".to_string(),
            display_name: "Gmail".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();

    let mut fact = home_fact(Some("10 Downing St"), None, None, None);
    fact.source_type = SourceType::Connector;
    fact.raw_reference = Some("gmail-msg-7".to_string());

    let outcome = normalize_and_insert(
        &kg,
        vec![fact],
        Provenance::connector(
            instance.id,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].source_fact_id, Some(outcome.inserted[0].id));
    assert!((locs[0].latitude.unwrap() - 51.5074).abs() < 1e-6);
}

#[tokio::test]
async fn upsert_location_facade_supersedes_directly() {
    let (kg, _dir) = fresh_kg().await;
    let devansh = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap();
    let from_2020 = parse_dt("2020-01-01T00:00:00Z");
    let from_2023 = parse_dt("2023-06-01T00:00:00Z");

    kg.upsert_location(
        devansh.id,
        LocationType::Home,
        Some("Old Road"),
        None,
        None,
        None,
        Some(from_2020),
        None,
        None,
    )
    .await
    .unwrap();
    kg.upsert_location(
        devansh.id,
        LocationType::Home,
        Some("New Road"),
        None,
        None,
        None,
        Some(from_2023),
        None,
        None,
    )
    .await
    .unwrap();

    let locs = kg.get_locations(devansh.id).await.unwrap();
    assert_eq!(locs.len(), 2);
    let old = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("Old Road"))
        .unwrap();
    assert_eq!(old.valid_until, Some(from_2023));
}
