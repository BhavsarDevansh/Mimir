//! Integration tests for the local-filesystem Photos connector (Phase 3 C1 /
//! issue #195): EXIF extraction, the incremental cursor, the live `notify`
//! push watcher, and the full supervisor → knowledge-graph path.
//!
//! Gated behind the `photos` feature (the connector + fixtures only exist
//! with it); `cargo test --no-default-features` skips this file entirely.

#![cfg(feature = "photos")]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;

use mimir_connectors::{
    ActionResult, Connector, ConnectorAction, ConnectorError, ConnectorFactory, ConnectorMode,
    ConnectorRegistry, ConnectorSupervisor, FnConnectorFactory, HealthStatus, PhotosConnector,
    PhotosConnectorFactory, PhotosCursor, SupervisorConfig, SyncOptions, SyncOutcome,
};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{
    ConnectorAuthState, ConnectorStatus, ConnectorType, LocationType,
};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::NormalizedFact;

use mimir_core::geocoder::{GeocodeResult, Geocoder, MockGeocoder};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    p
}

/// A connector config pointing at `watch_dir` with a tiny debounce for fast
/// watcher tests.
fn config_for(watch_dir: &std::path::Path, extra: serde_json::Value) -> serde_json::Value {
    let mut cfg = json!({
        "watch_dir": watch_dir.to_string_lossy(),
        "debounce_ms": 80,
        "__slug": "photos",
    });
    if let serde_json::Value::Object(map) = &mut cfg {
        if let serde_json::Value::Object(extra_map) = extra {
            for (k, v) in extra_map {
                map.insert(k, v);
            }
        }
    }
    cfg
}

fn make(config: serde_json::Value) -> Arc<PhotosConnector> {
    Arc::new(PhotosConnector::from_config(config).expect("connector constructs"))
}

// ---------------------------------------------------------------------------
// Initial scan + EXIF
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initial_scan_emits_photo_fact_with_gps() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(fixture("exif.jpg"), dir.path().join("IMG_001.jpg")).unwrap();

    let connector = make(config_for(dir.path(), json!({})));
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);

    let facts = connector.extract().await.unwrap();
    assert_eq!(facts.len(), 1, "one fact per photo");
    let fact = &facts[0];
    assert_eq!(fact.subject, "photos");
    // No geocoder → the coords-only `visited <coords-label>` fallback (issue
    // #250): the real-world event is the fact, the photo path is provenance.
    assert_eq!(fact.relationship_type, "visited");
    assert_eq!(fact.object, "46.500, 7.500");
    assert_eq!(fact.raw_reference.as_deref(), Some("IMG_001.jpg"));
    let loc = fact.location.as_ref().expect("GPS location overlay");
    assert_eq!(loc.location_type, LocationType::Visited);
    assert_eq!(loc.address, None);
    assert!((loc.latitude.unwrap() - 46.5).abs() < 1e-6);
    assert!((loc.longitude.unwrap() - 7.5).abs() < 1e-6);
    assert!(fact.valid_from.is_some());

    // The cursor now tracks the file, and is persisted via new_cursor.
    let cursor_json = outcome.new_cursor.expect("cursor advanced");
    let cursor = PhotosCursor::from_json(Some(&cursor_json)).unwrap();
    assert_eq!(cursor.len(), 1);
}

#[tokio::test]
async fn initial_scan_recurses_into_subdirs() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("2024/May")).unwrap();
    fs::copy(fixture("exif.tif"), dir.path().join("2024/May/pic.tif")).unwrap();

    let connector = make(config_for(dir.path(), json!({})));
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
    let facts = connector.extract().await.unwrap();
    assert_eq!(facts[0].object, "46.500, 7.500");
    assert_eq!(facts[0].relationship_type, "visited");
    assert_eq!(facts[0].raw_reference.as_deref(), Some("2024/May/pic.tif"));
}

#[tokio::test]
async fn non_image_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("notes.txt"), b"ignore me").unwrap();
    fs::copy(fixture("no_gps.jpg"), dir.path().join("photo.jpg")).unwrap();

    let connector = make(config_for(dir.path(), json!({})));
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1, "only the .jpg is staged");
}

// ---------------------------------------------------------------------------
// Incremental cursor
// ---------------------------------------------------------------------------

/// Build a cursor JSON marking `rel_path` as already processed with `sig`.
fn cursor_with(rel_path: &str, inode: u64, mtime_ms: i64, size: u64) -> String {
    // Build the persisted-cursor JSON shape directly so this test stays at
    // the public surface (PhotosCursor::upsert is crate-private).
    let json = serde_json::json!({
        "files": { rel_path: { "inode": inode, "mtime_ms": mtime_ms, "size": size } }
    });
    serde_json::to_string(&json).unwrap()
}

#[tokio::test]
async fn incremental_cursor_skips_unchanged_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(fixture("exif.jpg"), dir.path().join("IMG_001.jpg")).unwrap();
    let cfg = config_for(dir.path(), json!({}));

    // First run ingests the file and persists a cursor carrying its real
    // signature (inode + mtime + size).
    let first = make(cfg.clone());
    let outcome = first.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1);
    let cursor_json = outcome.new_cursor.expect("cursor persisted");
    // Drain the staged fact so it does not leak into the restart view.
    drop(first.extract().await);
    drop(first);

    // "Restart": a fresh connector seeded with the persisted cursor skips the
    // unchanged file (this is the C1 acceptance criterion).
    let restarted = make(config_for(dir.path(), json!({ "__cursor": cursor_json })));
    let outcome = restarted.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 0, "unchanged file skipped across restart");
    assert!(restarted.extract().await.unwrap().is_empty());
}

#[tokio::test]
async fn changed_file_is_reprocessed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("IMG_001.jpg");
    fs::copy(fixture("exif.jpg"), &path).unwrap();
    // Stale mtime in the cursor → the on-disk file looks changed.
    let cursor_json = cursor_with("IMG_001.jpg", 0, 0, 0);

    let connector = make(config_for(dir.path(), json!({ "__cursor": cursor_json })));
    let outcome = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(outcome.fetched, 1, "changed file reprocessed");
}

#[tokio::test]
async fn full_sync_ignores_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("IMG_001.jpg");
    fs::copy(fixture("exif.jpg"), &path).unwrap();
    let meta = fs::metadata(&path).unwrap();
    let mtime_ms = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let cursor_json = cursor_with("IMG_001.jpg", 0, mtime_ms, meta.len());

    let connector = make(config_for(dir.path(), json!({ "__cursor": cursor_json })));
    let outcome = connector
        .sync(SyncOptions {
            full: true,
            since: None,
        })
        .await
        .unwrap();
    assert_eq!(outcome.fetched, 1, "full sync re-ingests despite cursor");
}

// ---------------------------------------------------------------------------
// Live push watcher
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_watcher_stages_new_file() {
    let dir = tempfile::tempdir().unwrap();

    // First cycle: empty directory → nothing staged, watcher starts.
    let connector = make(config_for(dir.path(), json!({})));
    let first = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(first.fetched, 0);

    // Write a new image; the debounced watcher must surface it on a subsequent
    // blocking sync. notify can deliver a transient watcher-error or a
    // no-path event as the first event after a watch is established, so loop
    // over `sync` calls within a budget until the new file is staged (a missed
    // event fails the deadline loud rather than silently).
    fs::copy(fixture("exif.jpg"), dir.path().join("new.jpg")).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut fetched = 0u32;
    while fetched == 0 && tokio::time::Instant::now() < deadline {
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            connector.sync(SyncOptions::default()),
        )
        .await
        .expect("watcher delivered no event within 3s")
        .unwrap();
        fetched = outcome.fetched;
    }
    assert_eq!(fetched, 1, "new file staged via the push watcher");
    let facts = connector.extract().await.unwrap();
    assert_eq!(facts[0].object, "46.500, 7.500");
    assert_eq!(facts[0].relationship_type, "visited");
    assert_eq!(facts[0].raw_reference.as_deref(), Some("new.jpg"));
}

#[tokio::test]
async fn push_watcher_reprocesses_modified_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seed.jpg");
    fs::copy(fixture("exif.jpg"), &path).unwrap();

    let connector = make(config_for(dir.path(), json!({})));
    let first = connector.sync(SyncOptions::default()).await.unwrap();
    assert_eq!(first.fetched, 1);

    // Genuinely modify the file (new size + mtime). The debounced watcher must
    // surface it and the cursor must classify it as changed -> reprocessed.
    fs::write(&path, b"replaced bytes").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut fetched = 0u32;
    while fetched == 0 && tokio::time::Instant::now() < deadline {
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            connector.sync(SyncOptions::default()),
        )
        .await
        .expect("watcher delivered no event within 3s")
        .unwrap();
        fetched = outcome.fetched;
    }
    assert_eq!(fetched, 1, "modified file reprocessed via the push path");
}

// ---------------------------------------------------------------------------
// Full supervisor → knowledge-graph path
// ---------------------------------------------------------------------------

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn fast_config() -> SupervisorConfig {
    SupervisorConfig {
        max_failures: 5,
        base_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

#[tokio::test]
async fn supervisor_ingests_photo_into_kb_with_location() {
    let watch = tempfile::tempdir().unwrap();
    fs::copy(fixture("exif.jpg"), watch.path().join("IMG_001.jpg")).unwrap();

    let (kg, _db_dir) = init_kg().await;
    let config = serde_json::to_string(&json!({
        "watch_dir": watch.path().to_string_lossy(),
        "owner_name": "Devansh",
        "debounce_ms": 80,
    }))
    .unwrap();
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Photos,
            slug: "photos".to_string(),
            backend: "local".to_string(),
            display_name: "Photos".to_string(),
            config_json: config,
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Photos, "local", PhotosConnectorFactory)
        .unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx);
    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the owner entity + visited fact + persisted cursor.
    let kg2 = kg.clone();
    let row_id = row.id;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let Some(owner) = mimir_search_entity(&kg2, "Devansh").await else {
            assert!(
                tokio::time::Instant::now() < deadline,
                "owner entity never created"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let facts = kg2.get_facts_by_subject(owner, 100).await.unwrap();
        if facts
            .iter()
            .any(|f| f.object_literal.as_deref() == Some("46.500, 7.500"))
        {
            // Cursor is persisted by the supervisor in the same cycle, right
            // after the fact insert. Poll for it so a tiny commit-order gap
            // does not flake the test.
            let after = match kg2.get_connector(row_id).await.unwrap() {
                Some(c) if c.sync_cursor.is_some() => c,
                _ => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "cursor never persisted"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            assert_eq!(after.status(), Some(ConnectorStatus::Active));

            // Flush the async location-overlay worker, then assert the
            // GPS coordinates landed as an entity_locations row.
            kg2.flush_location_overlays().await;
            let locations = kg2.get_locations(owner).await.unwrap();
            assert!(
                locations.iter().any(|loc| {
                    loc.location_type_id == LocationType::Visited as i16
                        && loc.address.is_none()
                        && (loc.latitude.unwrap() - 46.5).abs() < 1e-6
                        && (loc.longitude.unwrap() - 7.5).abs() < 1e-6
                }),
                "no Visited GPS location row; got {locations:?}"
            );

            // Connector provenance.
            let fact = facts
                .iter()
                .find(|f| f.object_literal.as_deref() == Some("46.500, 7.500"))
                .unwrap();
            assert_eq!(
                kg2.relationship_type_name(fact.relationship_type_id)
                    .await
                    .as_deref(),
                Some("visited"),
                "coords-only photos author a `visited` fact, not a file-path object"
            );
            let sources = kg2.get_sources_for_fact(fact.id).await.unwrap();
            assert!(sources.iter().any(|s| {
                s.source_type_id == SourceType::Connector as i16
                    && s.connector_instance_id == Some(row_id)
                    && s.connector_type_id == Some(ConnectorType::Photos as i16)
                    && s.raw_reference.as_deref() == Some("IMG_001.jpg")
            }));

            supervisor.shutdown().await;
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "visited fact never landed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn mimir_search_entity(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
    kg.search_entities(name, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.entity.name == name)
        .map(|r| r.entity.id)
}

/// A mock geocoder that reverse-resolves any coordinate to "Rome" (the
/// fixture's GPS is 46.5, 7.5). The short name is the locality, so photos at
/// different spots in the city resolve to one `Rome` place entity.
fn rome_geocoder() -> Arc<dyn Geocoder> {
    Arc::new(MockGeocoder::new().with_reverse(Ok(Some(GeocodeResult {
        latitude: 46.5,
        longitude: 7.5,
        display_name: "Rome, Metropolitan City of Rome, Italy".to_string(),
        short_name: Some("Rome".to_string()),
        country: Some("Italy".to_string()),
        country_code: Some("it".to_string()),
        alternative_names: vec![],
    }))))
}

/// Find a `Place` entity by exact name (C2 / #196).
async fn find_place(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
    kg.search_entities(name, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.entity.name == name && r.entity.entity_type_id == EntityType::Place as i16)
        .map(|r| r.entity.id)
}

/// Register the Photos factory and spawn a supervisor that injects `geocoder`
/// into every connector it constructs (Phase 3 C2 / #196). Returns the
/// supervisor, the shared knowledge graph, the connector row id, and the
/// shutdown `watch` sender. The caller must hold the sender alive until
/// `supervisor.shutdown()` is called — dropping it first makes the runner
/// exit before it can ingest (the watch reports "all senders gone" as a
/// shutdown signal).
async fn setup_photos_supervisor(
    kg: KnowledgeGraph,
    watch_dir: &std::path::Path,
    owner: &str,
    geocoder: Arc<dyn Geocoder>,
) -> (
    ConnectorSupervisor,
    Arc<KnowledgeGraph>,
    i32,
    tokio::sync::watch::Sender<bool>,
) {
    let config = serde_json::to_string(&json!({
        "watch_dir": watch_dir.to_string_lossy(),
        "owner_name": owner,
        "debounce_ms": 80,
    }))
    .unwrap();
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Photos,
            slug: "photos".to_string(),
            backend: "local".to_string(),
            display_name: "Photos".to_string(),
            config_json: config,
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let kg = Arc::new(kg);
    let registry = ConnectorRegistry::new();
    registry
        .register(ConnectorType::Photos, "local", PhotosConnectorFactory)
        .unwrap();
    let (shutdown_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx)
        .with_geocoder(geocoder);
    assert_eq!(supervisor.restore().await.unwrap(), 1);
    (supervisor, kg, row.id, shutdown_tx)
}

/// A photo with GPS produces a `took_photo_at <place>` fact, a `Visited`
/// `entity_locations` row for the owner (coords + place name), and a
/// `Geographic` coordinate row anchoring the place entity (Phase 3 C2 / #196).
#[tokio::test]
async fn supervisor_ingests_photo_as_took_photo_at_place_fact() {
    let watch = tempfile::tempdir().unwrap();
    fs::copy(fixture("exif.jpg"), watch.path().join("IMG_001.jpg")).unwrap();

    let (kg, _db_dir) = init_kg().await;
    let (supervisor, kg, row_id, _shutdown_tx) =
        setup_photos_supervisor(kg, watch.path(), "Devansh", rome_geocoder()).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let (owner, place, fact) = loop {
        // The predicate is created on first ingestion (ensure_relationship_type),
        // so poll until the supervisor's first cycle registers it.
        let Some(took_photo_at) = kg.relationship_type_id("took_photo_at").await else {
            assert!(
                tokio::time::Instant::now() < deadline,
                "took_photo_at predicate never registered"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let Some(owner) = mimir_search_entity(&kg, "Devansh").await else {
            assert!(
                tokio::time::Instant::now() < deadline,
                "owner entity never created"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let Some(place) = find_place(&kg, "Rome").await else {
            assert!(
                tokio::time::Instant::now() < deadline,
                "Rome place entity never created"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let facts = kg
            .get_facts_by_subject_and_predicate(owner, took_photo_at)
            .await
            .unwrap();
        if let Some(fact) = facts.iter().find(|f| f.object_id == Some(place)) {
            break (owner, place, fact.clone());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "took_photo_at Rome fact never landed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // The place is the fact's object entity (no literal object).
    assert_eq!(fact.object_id, Some(place));
    assert!(fact.object_literal.is_none());

    // Connector provenance carries the photo's file path as the raw reference.
    let sources = kg.get_sources_for_fact(fact.id).await.unwrap();
    assert!(sources.iter().any(|s| {
        s.source_type_id == SourceType::Connector as i16
            && s.connector_instance_id == Some(row_id)
            && s.connector_type_id == Some(ConnectorType::Photos as i16)
            && s.raw_reference.as_deref() == Some("IMG_001.jpg")
    }));

    // Flush the async overlay worker, then assert both location rows.
    kg.flush_location_overlays().await;

    // Owner: a Visited row with the GPS coords and the place name as address.
    let owner_locs = kg.get_locations(owner).await.unwrap();
    assert!(
        owner_locs.iter().any(|loc| {
            loc.location_type_id == LocationType::Visited as i16
                && loc.address.as_deref() == Some("Rome")
                && (loc.latitude.unwrap() - 46.5).abs() < 1e-6
                && (loc.longitude.unwrap() - 7.5).abs() < 1e-6
        }),
        "no Visited owner location row with place name; got {owner_locs:?}"
    );

    // Place: a Geographic coordinate row anchoring Rome (Phase 3 C2 / #196).
    let place_locs = kg.get_locations(place).await.unwrap();
    assert!(
        place_locs.iter().any(|loc| {
            loc.location_type_id == LocationType::Geographic as i16
                && (loc.latitude.unwrap() - 46.5).abs() < 1e-6
                && (loc.longitude.unwrap() - 7.5).abs() < 1e-6
                && loc.source_fact_id == Some(fact.id)
        }),
        "no Geographic place anchor row; got {place_locs:?}"
    );

    supervisor.shutdown().await;
}

/// Delegating connector that fails the first `extract()` call, simulating a
/// transient extraction failure *after* `sync` already staged the photos.
/// Every other operation — including the cursor adoption in
/// `on_cycle_succeeded` — delegates to the inner photos connector, so the
/// wrapper only injects the failure (issue #332).
struct FailFirstExtractPhotosConnector {
    inner: Arc<dyn Connector>,
    /// Set once the injected extract failure has fired, so the test can wait
    /// for the failing cycle instead of racing the supervisor's first
    /// automatic cycle.
    failed_once: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Connector for FailFirstExtractPhotosConnector {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn connector_type(&self) -> ConnectorType {
        self.inner.connector_type()
    }
    fn mode(&self) -> ConnectorMode {
        self.inner.mode()
    }
    fn config_schema(&self) -> serde_json::Value {
        self.inner.config_schema()
    }
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        self.inner.authenticate().await
    }
    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        self.inner.health().await
    }
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        self.inner.sync(options).await
    }
    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        if !self.failed_once.swap(true, Ordering::SeqCst) {
            return Err(ConnectorError::Parse(
                "injected transient extract failure".to_string(),
            ));
        }
        self.inner.extract().await
    }
    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        self.inner.extract_deletions().await
    }
    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        self.inner.acknowledge_deletions(deleted).await
    }
    async fn on_cycle_succeeded(&self, new_cursor: Option<&str>) {
        self.inner.on_cycle_succeeded(new_cursor).await;
    }
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError> {
        self.inner.act(action).await
    }
    async fn forget(&self) -> Result<(), ConnectorError> {
        self.inner.forget().await
    }
}

/// Issue #332: a cycle that fails *after* `sync` (extract error) must not
/// lose the staged photos. The in-memory cursor may only advance once the
/// supervisor persisted the new cursor, so the next in-process cycle re-scans
/// from the last confirmed cursor (the file watcher does not re-deliver
/// consumed events) and re-processes the failed window.
#[tokio::test]
async fn failed_extract_cycle_reprocesses_staged_photos_on_next_cycle() {
    let watch = tempfile::tempdir().unwrap();
    fs::copy(fixture("exif.jpg"), watch.path().join("IMG_001.jpg")).unwrap();

    let (kg, _db_dir) = init_kg().await;
    let config = serde_json::to_string(&json!({
        "watch_dir": watch.path().to_string_lossy(),
        "owner_name": "Devansh",
        "debounce_ms": 80,
    }))
    .unwrap();
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Photos,
            slug: "photos".to_string(),
            backend: "local-failing".to_string(),
            display_name: "Photos".to_string(),
            config_json: config,
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let kg = Arc::new(kg);

    let failed_once = Arc::new(AtomicBool::new(false));
    let registry = ConnectorRegistry::new();
    registry
        .register(
            ConnectorType::Photos,
            "local-failing",
            FnConnectorFactory::new({
                let failed_once = Arc::clone(&failed_once);
                move |config, ctx| {
                    let inner = PhotosConnectorFactory.create(config, ctx)?;
                    Ok(Arc::new(FailFirstExtractPhotosConnector {
                        inner,
                        failed_once: Arc::clone(&failed_once),
                    }) as Arc<dyn Connector>)
                }
            }),
        )
        .unwrap();
    let (_shutdown_tx, rx) = tokio::sync::watch::channel(false);
    let supervisor = ConnectorSupervisor::new(Arc::new(registry), kg.clone(), fast_config(), rx);
    assert_eq!(supervisor.restore().await.unwrap(), 1);

    // Wait for the injected extract failure to fire, then for the retry
    // cycle to re-scan the failed window and land the photo's fact in the KB.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while !failed_once.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "injected extract failure never fired"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The retry cycle re-scans from the last confirmed cursor (none) and
    // re-processes the photo: the coords-only `visited` fact lands, and the
    // cursor is persisted by the supervisor in the same cycle, right after
    // the fact insert. Poll for both so a tiny commit-order gap does not
    // flake the test.
    let kg2 = kg.clone();
    let row_id = row.id;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let Some(owner) = mimir_search_entity(&kg2, "Devansh").await else {
            assert!(
                tokio::time::Instant::now() < deadline,
                "owner entity never created"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let facts = kg2.get_facts_by_subject(owner, 100).await.unwrap();
        if facts
            .iter()
            .any(|f| f.object_literal.as_deref() == Some("46.500, 7.500"))
        {
            let after = match kg2.get_connector(row_id).await.unwrap() {
                Some(c) if c.sync_cursor.is_some() => c,
                _ => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "cursor never persisted after the successful retry"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            assert_eq!(after.status(), Some(ConnectorStatus::Active));
            supervisor.shutdown().await;
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "visited fact never landed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
