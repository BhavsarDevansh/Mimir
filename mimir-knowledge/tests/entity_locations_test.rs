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
        short_name: Some("London".to_string()),
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
        event_type: None,
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
    // Location overlays are applied by a background worker, so drain the
    // worker's queue before reading to keep these integration tests
    // deterministic.
    kg.flush_location_overlays().await;
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

/// A "where" correction: `is_correction` with no scope defaults the inserted
/// fact's `valid_from` to `now`. The location overlay must use the *inserted
/// fact's* bounds (not the pre-correction `None`), so the new Home location is
/// dated at `now` and supersedes the prior open Home (regression test for the
/// pre-correction temporal-bounds bug).
#[tokio::test]
async fn correction_overlay_uses_corrected_bounds_and_supersedes() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_forward(Ok(Some(london_result()))),
    ));

    // Timeless open Home: "I live at Old Road" (no valid_from).
    normalize_and_insert(
        &kg,
        vec![home_fact(Some("Old Road"), None, None, None)],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    // Correction: "actually I live at New Road now" with no scope -> `now`.
    let mut correction = home_fact(Some("New Road"), None, None, None);
    correction.is_correction = true;
    correction.correction_scope = None;
    let outcome = normalize_and_insert(
        &kg,
        vec![correction],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);
    let now = outcome.inserted[0].valid_from;
    assert!(now.is_some(), "correction fact should be dated at `now`");

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 2, "both rows should coexist");
    let old = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("Old Road"))
        .unwrap();
    let new = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("New Road"))
        .unwrap();
    assert_eq!(
        old.valid_until, now,
        "prior open Home closed at the correction"
    );
    assert_eq!(new.valid_from, now, "new Home dated at the correction");
    assert!(new.valid_until.is_none(), "new location is open-ended");
}

/// A "where" correction with a datetime scope: the inserted fact's
/// `valid_from` becomes that datetime, and the overlay inherits it (not the
/// pre-correction `None`).
#[tokio::test]
async fn correction_overlay_with_datetime_scope_uses_scope_bounds() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_forward(Ok(Some(london_result()))),
    ));

    normalize_and_insert(
        &kg,
        vec![home_fact(Some("Old Road"), None, None, None)],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    let scope = parse_dt("2024-01-01T00:00:00Z");
    let mut correction = home_fact(Some("New Road"), None, None, None);
    correction.is_correction = true;
    correction.correction_scope = Some(scope.to_rfc3339());
    let outcome = normalize_and_insert(
        &kg,
        vec![correction],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(outcome.inserted[0].valid_from, Some(scope));

    let locs = subject_locations(&kg, outcome.inserted[0].subject_id).await;
    assert_eq!(locs.len(), 2);
    let old = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("Old Road"))
        .unwrap();
    let new = locs
        .iter()
        .find(|l| l.address.as_deref() == Some("New Road"))
        .unwrap();
    assert_eq!(old.valid_until, Some(scope));
    assert_eq!(new.valid_from, Some(scope));
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

/// A batch of location facts is enqueued to the background worker and all
/// overlays are persisted after a single flush (regression guard for the
/// fire-and-forget worker that un-gates ingestion from the geocoder rate
/// limit).
#[tokio::test]
async fn batch_of_location_facts_persisted_after_flush() {
    let (mut kg, _dir) = fresh_kg().await;
    kg.set_geocoder(Arc::new(
        MockGeocoder::new().with_forward(Ok(Some(london_result()))),
    ));

    let facts: Vec<NormalizedFact> = (0..5)
        .map(|i| {
            let mut f = home_fact(
                Some(&format!("Address {i}")),
                None,
                None,
                Some(parse_dt(&format!("202{i}-01-01T00:00:00Z"))),
            );
            // Distinct subjects so each is a standalone Visited-style fix;
            // use a per-fact subject so rows don't supersede each other.
            f.subject = format!("Person {i}");
            f.location.as_mut().unwrap().location_type = LocationType::Visited;
            f
        })
        .collect();

    let outcome = normalize_and_insert(
        &kg,
        facts,
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 5);

    kg.flush_location_overlays().await;
    for fact in &outcome.inserted {
        let locs = kg.get_locations(fact.subject_id).await.unwrap();
        assert_eq!(locs.len(), 1, "one location per subject");
        assert_eq!(locs[0].source_fact_id, Some(fact.id));
        assert!(
            locs[0].latitude.is_some(),
            "forward-geocoded coords present"
        );
    }
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

/// A `Place` entity gets exactly one `Geographic` coordinate row, even when
/// `ensure_place_coordinates` is called repeatedly (and concurrently) for the
/// same place — the partial unique index (migration 047) backs the
/// single-row invariant at the schema level (Phase 3 C2 / #196 review fix).
#[tokio::test]
async fn ensure_place_coordinates_keeps_single_geographic_row() {
    use mimir_knowledge::queries::entity::ensure_place_coordinates;

    let (kg, _dir) = fresh_kg().await;
    let place = kg
        .create_entity("London", EntityType::Place, &[])
        .await
        .unwrap();

    // Two sequential anchors at slightly different coordinates update in place.
    ensure_place_coordinates(kg.pool(), place.id, 51.5074, -0.1278, None)
        .await
        .unwrap();
    ensure_place_coordinates(kg.pool(), place.id, 51.5075, -0.1279, None)
        .await
        .unwrap();

    // Two concurrent anchors for the same place must not duplicate the row —
    // the ON CONFLICT upsert is atomic against the partial unique index.
    // Each task owns its own pool clone and borrows it for the call.
    let pool = kg.pool().clone();
    let pool_a = pool.clone();
    let a = tokio::spawn(async move {
        ensure_place_coordinates(&pool_a, place.id, 51.51, -0.13, None).await
    });
    let pool_b = pool.clone();
    let b = tokio::spawn(async move {
        ensure_place_coordinates(&pool_b, place.id, 51.52, -0.14, None).await
    });
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();

    let locs = kg.get_locations(place.id).await.unwrap();
    let geographic: Vec<_> = locs
        .iter()
        .filter(|l| l.location_type_id == LocationType::Geographic as i16)
        .collect();
    assert_eq!(
        geographic.len(),
        1,
        "place should have exactly one Geographic row; got {geographic:?}"
    );
    // The last writer wins (both concurrent calls complete; exact final coords
    // are nondeterministic, so only assert the row is one of the two writes).
    let row = geographic[0];
    assert!(
        (row.latitude.unwrap() - 51.5).abs() < 0.1,
        "coords should be near the anchored values; got {row:?}"
    );
}
