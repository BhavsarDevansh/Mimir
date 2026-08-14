use super::*;

use std::fs;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use mimir_core::geocoder::{GeocodeResult, MockGeocoder};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::LocationType;

use crate::connector::{Connector, ConnectorContext, ConnectorFactory, SyncOptions};
use crate::photos::PhotosConnectorFactory;

use super::logic_tests::fixture;

/// A `MockGeocoder` reverse result for (46.5, 7.5) → "Rome".
fn rome_geocoder() -> MockGeocoder {
    MockGeocoder::new().with_reverse(Ok(Some(GeocodeResult {
        latitude: 46.5,
        longitude: 7.5,
        display_name: "Rome, Metropolitan City of Rome, Italy".to_string(),
        short_name: Some("Rome".to_string()),
        country: Some("Italy".to_string()),
        country_code: Some("it".to_string()),
        alternative_names: vec![],
    })))
}

fn gps_raw(rel_path: &str, lat: f64, lng: f64) -> RawPhoto {
    RawPhoto {
        rel_path: rel_path.to_string(),
        taken_at: DateTime::<Utc>::from_timestamp(1_715_000_000, 0).unwrap(),
        latitude: Some(lat),
        longitude: Some(lng),
    }
}

#[tokio::test]
async fn extract_reverse_geocodes_gps_into_took_photo_at_fact() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "owner_name": "Devansh",
    });
    let connector = PhotosConnector::from_config_with_geocoder(
        config,
        Some(Arc::new(rome_geocoder()) as Arc<dyn mimir_core::geocoder::Geocoder>),
        None,
    )
    .unwrap();
    // Stage two photos at the same spot to exercise the coord-dedup cache.
    connector
        .buffer
        .lock()
        .await
        .extend([gps_raw("a.jpg", 46.5, 7.5), gps_raw("b.jpg", 46.5, 7.5)]);
    let facts = connector.extract().await.unwrap();
    assert_eq!(facts.len(), 2);
    for fact in &facts {
        assert_eq!(fact.relationship_type, "took_photo_at");
        assert_eq!(fact.object, "Rome");
        assert!(fact.object_is_entity);
        assert_eq!(fact.object_type, Some(EntityType::Place));
        assert_eq!(
            fact.location.as_ref().unwrap().address.as_deref(),
            Some("Rome")
        );
    }
    // One cache entry for the shared ~100 m bucket, not one per photo.
    assert_eq!(connector.geocode_cache.lock().await.len(), 1);
}

/// The canonical user identity injected via `ConnectorContext::user_identity`
/// (issue #246) wins over the per-instance `owner_name` config field, so
/// photo facts resolve to the same `Person` entity the daemon resolves as
/// `user_entity_id`.
#[tokio::test]
async fn extract_authors_facts_with_canonical_user_identity() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "owner_name": "my-photos",
    });
    let connector = PhotosConnector::from_config_with_geocoder(
        config,
        Some(Arc::new(rome_geocoder()) as Arc<dyn mimir_core::geocoder::Geocoder>),
        Some("Devansh".to_string()),
    )
    .unwrap();
    connector
        .buffer
        .lock()
        .await
        .push(gps_raw("a.jpg", 46.5, 7.5));
    let fact = connector.extract().await.unwrap().pop().unwrap();
    assert_eq!(fact.relationship_type, "took_photo_at");
    assert_eq!(fact.subject, "Devansh");
    assert_eq!(fact.subject_type, EntityType::Person);
}

/// Without an injected identity, the per-instance `owner_name` remains the
/// subject fallback (mirroring the Calendar connector's `None`-identity
/// fallback), so a library without a configured `[identity] name` still
/// produces facts.
#[tokio::test]
async fn extract_falls_back_to_owner_name_without_identity() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "owner_name": "my-photos",
    });
    let connector = PhotosConnector::from_config_with_geocoder(config, None, None).unwrap();
    connector
        .buffer
        .lock()
        .await
        .push(gps_raw("a.jpg", 46.5, 7.5));
    let fact = connector.extract().await.unwrap().pop().unwrap();
    assert_eq!(fact.subject, "my-photos");
    assert_eq!(fact.subject_type, EntityType::Person);
}

/// The factory must forward `ConnectorContext::user_identity` into the
/// connector (as `CalendarConnectorFactory` does), so a daemon-configured
/// `[identity] name` reaches photo facts end-to-end.
#[tokio::test]
async fn factory_forwards_user_identity_into_connector() {
    let dir = tempfile::tempdir().unwrap();
    let watch_dir = dir.path().to_path_buf();
    fs::copy(fixture("exif.jpg"), watch_dir.join("exif.jpg")).unwrap();

    let config = serde_json::json!({
        "watch_dir": watch_dir.to_string_lossy(),
        "owner_name": "my-photos",
    });
    let ctx = ConnectorContext::empty().with_user_identity("Devansh");
    let connector = PhotosConnectorFactory.create(config, &ctx).unwrap();
    connector.sync(SyncOptions::default()).await.unwrap();
    let facts = connector.extract().await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].subject, "Devansh");
    assert_eq!(facts[0].subject_type, EntityType::Person);
}

#[tokio::test]
async fn extract_falls_back_when_geocoder_finds_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "owner_name": "Devansh",
    });
    let geocoder = MockGeocoder::new().with_reverse(Ok(None));
    let connector = PhotosConnector::from_config_with_geocoder(
        config,
        Some(Arc::new(geocoder) as Arc<dyn mimir_core::geocoder::Geocoder>),
        None,
    )
    .unwrap();
    connector
        .buffer
        .lock()
        .await
        .push(gps_raw("a.jpg", 0.0, 0.0));
    let fact = connector.extract().await.unwrap().pop().unwrap();
    // No place → C1 coords-only `took_photo` fallback; data is not lost.
    assert_eq!(fact.relationship_type, "took_photo");
    assert_eq!(fact.object, "a.jpg");
    assert!(!fact.object_is_entity);
    assert!(fact.location.is_some());
    // A genuine no-match is cached so the same spot is not re-queried.
    assert_eq!(connector.geocode_cache.lock().await.len(), 1);
}

/// A `Geocoder` that always errors on `reverse`, counting calls so the
/// per-cycle failed-key short-circuit can be asserted (Phase 3 C2 / #196
/// review fix: a sustained outage must not retry per photo).
#[derive(Debug)]
struct FailingGeocoder {
    reverse_calls: Arc<std::sync::atomic::AtomicU64>,
}

impl FailingGeocoder {
    fn new(counter: Arc<std::sync::atomic::AtomicU64>) -> Self {
        Self {
            reverse_calls: counter,
        }
    }
}

#[async_trait::async_trait]
impl mimir_core::geocoder::Geocoder for FailingGeocoder {
    async fn forward(
        &self,
        _query: &str,
    ) -> Result<Option<GeocodeResult>, mimir_core::geocoder::GeocodeError> {
        Ok(None)
    }
    async fn reverse(
        &self,
        _latitude: f64,
        _longitude: f64,
    ) -> Result<Option<GeocodeResult>, mimir_core::geocoder::GeocodeError> {
        self.reverse_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(mimir_core::geocoder::GeocodeError::Network(
            "simulated outage".to_string(),
        ))
    }
}

#[tokio::test]
async fn extract_bounds_geocode_retries_to_one_per_spot_per_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "owner_name": "Devansh",
    });
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let geocoder: Arc<dyn mimir_core::geocoder::Geocoder> =
        Arc::new(FailingGeocoder::new(counter.clone()));
    let connector =
        PhotosConnector::from_config_with_geocoder(config, Some(geocoder), None).unwrap();
    // Three photos at the same ~100 m bucket, plus one at a different spot.
    connector.buffer.lock().await.extend([
        gps_raw("a.jpg", 46.5001, 7.5001),
        gps_raw("b.jpg", 46.5002, 7.5002),
        gps_raw("c.jpg", 46.5003, 7.5003),
        gps_raw("d.jpg", 1.0, 1.0),
    ]);
    let facts = connector.extract().await.unwrap();
    // All four photos degrade to the C1 coords-only fallback; no data lost.
    assert_eq!(facts.len(), 4);
    for fact in &facts {
        assert_eq!(fact.relationship_type, "took_photo");
        assert!(!fact.object_is_entity);
    }
    // One geocode attempt per distinct bucket (2), not per photo (4): the
    // per-cycle failed-key set short-circuits repeat attempts for the spot
    // that already errored. Transient errors are not cached long-lived, so
    // the geocode_cache stays empty.
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(connector.geocode_cache.lock().await.len(), 0);
    // A fresh extract() cycle retries the failed buckets (per-cycle scope).
    connector.buffer.lock().await.extend([
        gps_raw("e.jpg", 46.5001, 7.5001),
        gps_raw("f.jpg", 1.0, 1.0),
    ]);
    let before = counter.load(std::sync::atomic::Ordering::SeqCst);
    connector.extract().await.unwrap();
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        before + 2,
        "next cycle should retry the two buckets"
    );
}

#[tokio::test]
async fn extract_falls_back_without_geocoder() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "owner_name": "Devansh",
    });
    let connector = PhotosConnector::from_config(config).unwrap();
    connector
        .buffer
        .lock()
        .await
        .push(gps_raw("a.jpg", 46.5, 7.5));
    let fact = connector.extract().await.unwrap().pop().unwrap();
    assert_eq!(fact.relationship_type, "took_photo");
    assert_eq!(fact.object, "a.jpg");
}

// -- fact conversion --
#[test]
fn raw_photo_with_gps_falls_back_to_coords_only_without_place() {
    let raw = RawPhoto {
        rel_path: "2024/IMG_001.jpg".to_string(),
        taken_at: DateTime::<Utc>::from_timestamp(1_715_000_000, 0).unwrap(),
        latitude: Some(46.5),
        longitude: Some(7.5),
    };
    // No resolved place → C1 coords-only `took_photo` fallback shape.
    let fact = raw.to_fact("Devansh", None);
    assert_eq!(fact.subject, "Devansh");
    assert_eq!(fact.subject_type, EntityType::Person);
    assert_eq!(fact.relationship_type, "took_photo");
    assert_eq!(fact.object, "2024/IMG_001.jpg");
    assert!(!fact.object_is_entity);
    assert_eq!(fact.raw_reference.as_deref(), Some("2024/IMG_001.jpg"));
    let loc = fact.location.expect("location overlay");
    assert_eq!(loc.location_type, LocationType::Visited);
    assert_eq!(loc.address, None);
    assert!((loc.latitude.unwrap() - 46.5).abs() < 1e-9);
    assert!((loc.longitude.unwrap() - 7.5).abs() < 1e-9);
}

#[test]
fn raw_photo_with_resolved_place_emits_took_photo_at_fact() {
    let raw = RawPhoto {
        rel_path: "2024/IMG_001.jpg".to_string(),
        taken_at: DateTime::<Utc>::from_timestamp(1_715_000_000, 0).unwrap(),
        latitude: Some(46.5),
        longitude: Some(7.5),
    };
    // A resolved locality name → `took_photo_at <place>` with the place as
    // a Place object entity and a location overlay carrying coords + the
    // place name (Phase 3 C2 / #196).
    let fact = raw.to_fact("Devansh", Some("Rome".to_string()));
    assert_eq!(fact.relationship_type, "took_photo_at");
    assert_eq!(fact.object, "Rome");
    assert!(fact.object_is_entity);
    assert_eq!(fact.object_type, Some(EntityType::Place));
    // The photo's file path is preserved as the native source id.
    assert_eq!(fact.raw_reference.as_deref(), Some("2024/IMG_001.jpg"));
    let loc = fact.location.expect("location overlay");
    assert_eq!(loc.location_type, LocationType::Visited);
    assert_eq!(loc.address.as_deref(), Some("Rome"));
    assert!((loc.latitude.unwrap() - 46.5).abs() < 1e-9);
    assert!((loc.longitude.unwrap() - 7.5).abs() < 1e-9);
}

#[test]
fn raw_photo_without_gps_has_no_location_overlay() {
    let raw = RawPhoto {
        rel_path: "no_gps.jpg".to_string(),
        taken_at: DateTime::<Utc>::from_timestamp(1_715_000_000, 0).unwrap(),
        latitude: None,
        longitude: None,
    };
    let fact = raw.to_fact("Devansh", None);
    assert!(fact.location.is_none());
}

// -- watcher init / first-scan failure recovery (PR #232 review) --
/// A failed `start_watcher` (watch dir vanishes before the first `sync`)
/// must leave `started == false` so the supervisor's retry re-runs setup
/// instead of no-op'ing and busy-looping on the closed event channel.
#[tokio::test]
async fn start_watcher_failure_leaves_started_false() {
    let dir = tempfile::tempdir().unwrap();
    let watch_dir = dir.path().to_path_buf();
    let config = serde_json::json!({ "watch_dir": watch_dir.to_string_lossy() });
    let connector = PhotosConnector::from_config(config).unwrap();

    // Watch dir vanishes between construction and the first `sync`.
    fs::remove_dir_all(&watch_dir).unwrap();
    assert!(connector.start_watcher().await.is_err());
    assert!(!connector.started.load(Ordering::SeqCst));

    // A second attempt must not short-circuit on a stale `started` flag.
    assert!(connector.start_watcher().await.is_err());
    assert!(!connector.started.load(Ordering::SeqCst));

    // Once the dir reappears, setup succeeds and the flag is flipped.
    fs::create_dir_all(&watch_dir).unwrap();
    assert!(connector.start_watcher().await.is_ok());
    assert!(connector.started.load(Ordering::SeqCst));
}

/// A failed first `initial_scan` must restore `first_cycle` so the
/// supervisor's retry re-runs the initial recursive scan instead of
/// skipping it and missing every pre-existing file until a restart.
#[tokio::test]
async fn failed_initial_scan_restores_first_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let watch_dir = dir.path().to_path_buf();
    // Seed one image so a successful scan stages exactly one file.
    fs::copy(fixture("exif.jpg"), watch_dir.join("exif.jpg")).unwrap();

    let config = serde_json::json!({ "watch_dir": watch_dir.to_string_lossy() });
    let connector = PhotosConnector::from_config(config).unwrap();

    // Install the watcher up front so `sync`'s `start_watcher` is a
    // no-op; the only thing that can fail is the initial scan.
    assert!(connector.start_watcher().await.is_ok());
    assert!(connector.first_cycle.load(Ordering::SeqCst));

    // Root becomes unreadable between construction and the first cycle.
    fs::remove_dir_all(&watch_dir).unwrap();
    assert!(connector.sync(SyncOptions::default()).await.is_err());

    // The flag was restored, so the retry re-runs the scan.
    assert!(connector.first_cycle.load(Ordering::SeqCst));

    // Root reappears with the pre-existing image; the retry must ingest it.
    fs::create_dir_all(&watch_dir).unwrap();
    fs::copy(fixture("exif.jpg"), watch_dir.join("exif.jpg")).unwrap();
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
}
