//! Integration tests for the entity-locations proximity query
//! `KnowledgeGraph::find_nearby` (Phase 3 S4 / issue #194).
//!
//! Verifies the bounding-box SQL pre-filter plus exact Haversine post-filter:
//! points inside the radius are returned sorted nearest-first, edge-of-box
//! points outside the radius are excluded, locations without coordinates are
//! skipped, and optional temporal scoping honours `valid_from`/`valid_until`.

use chrono::{DateTime, Utc};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::LocationType;

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone::<Utc>(&Utc)
}

async fn fresh_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("nearby.db"))
        .await
        .unwrap();
    (kg, dir)
}

/// Seed a location for a freshly-created entity and return its row id.
async fn seed(
    kg: &KnowledgeGraph,
    name: &str,
    location_type: LocationType,
    lat: f64,
    lon: f64,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> i32 {
    let entity = kg
        .create_entity(name, EntityType::Place, &[])
        .await
        .unwrap();
    kg.insert_location(
        entity.id,
        location_type,
        Some(name),
        Some(lat),
        Some(lon),
        None,
        valid_from,
        valid_until,
        None,
    )
    .await
    .unwrap();
    entity.id
}

#[tokio::test]
async fn points_within_radius_returned_nearest_first() {
    let (kg, _dir) = fresh_kg().await;
    // London 51.5074, -0.1278 as the query point.
    seed(
        &kg,
        "Westminster",
        LocationType::Current,
        51.5074,
        -0.1278,
        None,
        None,
    )
    .await;
    // ~1.3 km north
    seed(
        &kg,
        "Camden",
        LocationType::Visited,
        51.5190,
        -0.1278,
        None,
        None,
    )
    .await;
    // ~5.5 km east
    seed(
        &kg,
        "Canary Wharf",
        LocationType::Work,
        51.5050,
        -0.0500,
        None,
        None,
    )
    .await;

    let near = kg.find_nearby(51.5074, -0.1278, 10.0, None).await.unwrap();

    assert_eq!(near.len(), 3, "all three are within 10 km");
    // Nearest first: Westminster (0) < Camden (~1.3) < Canary Wharf (~5.5).
    assert_eq!(near[0].location.address.as_deref(), Some("Westminster"));
    assert!(
        near[0].distance_km < 0.01,
        "coincident point should be ~0 km"
    );
    assert!(
        near[1].distance_km < near[2].distance_km,
        "sorted ascending"
    );
    assert!(near.iter().all(|n| n.distance_km <= 10.0));
}

#[tokio::test]
async fn points_outside_radius_excluded_even_if_inside_bbox() {
    let (kg, _dir) = fresh_kg().await;
    seed(&kg, "Near", LocationType::Current, 0.0, 0.0, None, None).await;
    // A point inside the 10 km bounding box (±0.0898° at the equator) but
    // beyond the 10 km radius: (0.07, 0.07) is ~11.0 km away, so it must be
    // dropped by the exact Haversine post-filter, not the coarse box.
    seed(&kg, "Edge", LocationType::Visited, 0.07, 0.07, None, None).await;

    let near = kg.find_nearby(0.0, 0.0, 10.0, None).await.unwrap();
    assert_eq!(near.len(), 1, "only the within-radius point");
    assert_eq!(near[0].location.address.as_deref(), Some("Near"));
}

#[tokio::test]
async fn locations_without_coords_skipped() {
    let (kg, _dir) = fresh_kg().await;
    let entity = kg
        .create_entity("Address-only", EntityType::Place, &[])
        .await
        .unwrap();
    kg.insert_location(
        entity.id,
        LocationType::Home,
        Some("10 Downing St"),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    seed(
        &kg,
        "Geocoded",
        LocationType::Current,
        51.5,
        -0.12,
        None,
        None,
    )
    .await;

    let near = kg.find_nearby(51.5, -0.12, 50.0, None).await.unwrap();
    assert_eq!(near.len(), 1);
    assert_eq!(near[0].location.address.as_deref(), Some("Geocoded"));
}

#[tokio::test]
async fn temporal_scope_filters_by_validity() {
    let (kg, _dir) = fresh_kg().await;
    let now = parse_dt("2024-06-01T00:00:00Z");
    // A "previous home": valid 2020–2023, closed before `now`.
    seed(
        &kg,
        "Old Home",
        LocationType::Home,
        51.5,
        -0.12,
        Some(parse_dt("2020-01-01T00:00:00Z")),
        Some(parse_dt("2023-01-01T00:00:00Z")),
    )
    .await;
    // A current home: open-ended from 2023.
    seed(
        &kg,
        "New Home",
        LocationType::Home,
        51.51,
        -0.12,
        Some(parse_dt("2023-01-01T00:00:00Z")),
        None,
    )
    .await;

    // At `now`, only the open-ended home is valid.
    let at_now = kg.find_nearby(51.5, -0.12, 50.0, Some(now)).await.unwrap();
    assert_eq!(at_now.len(), 1);
    assert_eq!(at_now[0].location.address.as_deref(), Some("New Home"));

    // With no temporal scope, both are returned.
    let all = kg.find_nearby(51.5, -0.12, 50.0, None).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn empty_radius_returns_nothing() {
    let (kg, _dir) = fresh_kg().await;
    seed(
        &kg,
        "Somewhere",
        LocationType::Current,
        51.5,
        -0.12,
        None,
        None,
    )
    .await;
    let near = kg.find_nearby(0.0, 0.0, 1.0, None).await.unwrap();
    assert!(near.is_empty());
}
