# Photos Connector (local filesystem) — `mimir-connectors::photos`

> **Phase:** 3 — Connectors (C1 / issue #195)
> **Feature flag:** `photos` (default). Framework + mock stay built without it.
> **Status:** Implemented (library + daemon/CLI integration). Daemon `AppState` wiring (A1 / #202), action routes (A2 / #203), and the `mimir connector …` CLI (A3 / #204) are integrated; C2 (GPS → place reverse-geocoding + `entity_locations` enrichment) is #196.
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

The Photos connector is the first concrete connector backend built on the
F6–F13 framework. It watches a configured local directory recursively for
image files, extracts EXIF GPS + datetime metadata with `kamadak-exif`, and
emits one fact per photo through the shared `normalize_and_insert` pipeline
(the supervisor owns the insert). It is read-only, push-mode, no-network, and
needs no authentication.

C1 (#195) emitted a coords-only `took_photo <rel_path>` literal-object fact.
C2 (#196) reverse-geocodes the EXIF GPS into a locality-level place name so
the fact becomes `owner took_photo_at <place>` (the place is a `Place` object
entity). This enables entity resolution and cross-photo corroboration: photos
taken at different spots in the same city resolve to one place entity and
merge into a single corroborated fact rather than fragmenting per photo. The
connector makes no network calls itself — the reverse geocode reuses the
shared `Geocoder` injected via the [`ConnectorContext`](#geocoder-injection).

## Dependencies

All optional, gated by the `photos` feature (verified on crates.io against
the issue's pinned versions):

| Crate | Version | Role |
|-------|---------|------|
| `notify` | 8.2 (latest stable 8.x) | Cross-platform `RecommendedWatcher` filesystem watcher. |
| `notify-debouncer-full` | 0.7 (depends on `notify ^8.2.0`) | Debounced event delivery + `FileIdMap`/`RecommendedCache` that coalesces rename/modify bursts. |
| `kamadak-exif` | 0.6 (latest stable) | Pure-Rust EXIF reader for JPEG/TIFF/HEIF/PNG/WebP containers ("across camera formats"). |

`notify`/`notify-debouncer-full`'s stable line has no tokio feature, so the
connector implements `notify_debouncer_full::DebounceEventHandler` on a small
newtype wrapping a `tokio::sync::mpsc::UnboundedSender`. The debouncer runs the
handler on its own thread and drives the channel with a synchronous `send`; no
forwarder thread and no extra async-runtime feature is required.

## Ingestion model

The connector runs in `ConnectorMode::Push`:

1. **First `sync()`** — the supervisor starts the connector, the connector
   starts the debounced watcher, then runs an **initial recursive scan** of the
   watch directory. Every image file whose signature (inode + mtime + size) is
   not already in the persisted cursor is staged. On a fresh cursor (first-ever
   run) this ingests the whole library; after a restart it skips already-
   processed files. Deleted entries are pruned so the cursor tracks the live
   library.
2. **Subsequent `sync()` calls** block on the notify event channel until
   debounced filesystem events arrive, then stage only the new/changed image
   files. The supervisor loops immediately after a successful push cycle, so
   `sync()` is the "wait for events" blocking point.
3. `extract()` drains the staged raw photos into typed `NormalizedFact`s.
   The buffer guard is released *before* the per-photo reverse-geocode loop
   (the buffer is `std::mem::take`-drained into a local `Vec` under the lock),
   so the buffer mutex is never held across `geocoder.reverse()` awaits.

Because the connector never touches the database, it stays `sqlx`-free and is
unit-testable without a live knowledge graph (see `tests/photos_connector.rs`).

## Incremental cursor

The cursor is a per-file signature map `path -> {inode, mtime, size}`
(`PhotosCursor`), serialised to JSON and persisted by the supervisor in the
`connectors` table's `sync_cursor` column. At construction the supervisor
injects the persisted cursor into the connector's `config_json` as `__cursor`
(see [Cursor injection](#cursor-injection)); the connector seeds its in-memory
map from it.

A file is **unchanged** iff its path is present with a matching signature;
**new** if the path is absent; **changed** if the path exists but the signature
differs. `sync()` reports `new_cursor = Some(json)` only when the cursor
actually moved, so an unchanged push cycle returns `None` and the supervisor
just stamps `last_sync_at` (the nullable-cursor contract). `SyncOptions::full`
clears the cursor first → re-ingests everything.

The cursor is O(N) in library size and rewritten on each change; acceptable for
V1 (syncs are infrequent). A dedicated/compacted cursor table is future work.

## Cursor injection

For incremental connectors to skip already-processed files across restarts
they must read the *previous* cursor, but the `Connector::sync(SyncOptions)`
surface carries no cursor and the connector never touches the DB. The
supervisor's `instantiate` therefore injects the persisted `sync_cursor` into
the connector's `config_json` as `__cursor` (alongside the existing `__slug`,
`__ctype`, `__instance_id` identity keys). A `None` cursor is injected as JSON
`null`, which the connector interprets as "no prior cursor" (a full first scan).

This is a small, internal extension to the F8 supervisor (issue #185) and
follows the existing identity-injection pattern; it is the read side that
complements `KnowledgeGraph::update_sync_cursor` (the write side).

## Fact shape

Each photo becomes one fact. The shape depends on whether a place could be
resolved from the GPS:

### Place fact (C2, GPS resolved)

When the reverse geocode yields a locality-level place name:

- **Subject** — the configured owner display name (`Person`); defaults to the
  connector slug. `owner_name` is a `config_json` field.
- **Predicate** — `took_photo_at`.
- **Object** — the resolved place short name (a `Place` object entity; the
  pipeline resolves or creates it via the full entity-resolution chain).
- **Temporal** — `valid_from` = the EXIF `DateTimeOriginal` (with `OffsetTime*`
  if present, otherwise interpreted as UTC); falls back to the file mtime when
  EXIF has no datetime. `valid_until` = `None` (open-ended), so a second photo
  at the same place temporally overlaps and corroborates the first instead of
  creating a new fact row.
- **Location overlay** — `NormalizedLocation { location_type: Visited,
  address: <place name>, latitude, longitude, timezone: None }`. The pipeline
  writes a `Visited` `entity_locations` row for the **owner** (carrying the
  coords and the place name as its address) and, because the object is a
  `Place`, anchors the **place entity's own** coordinates in a `Geographic`
  row (see [Place-coordinate anchoring](#place-coordinate-anchoring)).
- **Provenance** — `SourceType::Connector`, `ConnectorType::Photos`,
  `ExtractionMethod::StructuredParse` (set by the supervisor),
  `raw_reference` = the photo's watch-dir-relative path.

### Coords-only fallback (C1 shape)

When there is no geocoder, no GPS, a genuine no-match, or a transient geocode
error, the fact degrades to the C1 shape so no data is lost:

- **Predicate** — `took_photo`.
- **Object** — the photo's watch-dir-relative path (literal).
- **Location overlay** — `NormalizedLocation { location_type: Visited,
  address: None, latitude, longitude, timezone: None }` (coords only).
- Everything else (subject, temporal, provenance) as above.

Files with no GPS produce a fact with no location overlay; files with no EXIF
use the file mtime. Non-image files are skipped at the extension filter
(default `.jpg .jpeg .tif .tiff .png .heif .heic .webp`; configurable). RAW
formats (CR2/ARW/NEF) are deferred (they need a dedicated raw-EXIF reader).

### Corroboration and scale

A photo is a **fact**, not an entity. The only entities created are the owner
(`Person`) and one `Place` per distinct locality Mimir sees. Because the
place fact is open-ended, the shared corroboration path (the same one chat
facts use) detects a second photo at the same place as the same claim
(same subject + predicate + object, temporally overlapping) and **merges it
into the existing fact** — adding a `source` row (one per photo, carrying its
`raw_reference` path) and boosting confidence +0.05, capped at 0.95. Photos
base confidence is 0.80; two photos at the same place → one fact at 0.85.

So knowledge-graph growth is O(distinct places), not O(photos). The only
per-photo storage is the lightweight `source` provenance row — the trail a
future on-demand photo search walks to reach the actual file. POI-level
detail (the specific restaurant/landmark) is deliberately not stored as an
entity in C2; it remains available via the geocoder's `display_name` for a
future query-time reverse geocode (tracked as a follow-up).

<a id="geocoder-injection"></a>

## Geocoder injection (C2 / #196)

The reverse geocode reuses the shared `Geocoder` (Phase 3 S1 / #191) rather
than the connector holding its own HTTP client. The geocoder is injected at
construction through a new `ConnectorContext` — a small shared-services struct
threaded factory → registry → supervisor:

1. `ConnectorFactory::create(config, ctx)` now receives a `&ConnectorContext`.
   `ConnectorRegistry::create_with_context` forwards it; the config-only
   `create` shortcut delegates with an empty context.
2. `ConnectorSupervisor::with_geocoder(Arc<dyn Geocoder>)` sets the context's
   geocoder; `instantiate` calls `create_with_context` so every connector the
   supervisor spawns can read it. The daemon will set this from
   `KnowledgeGraph::geocoder()` once the server wires a supervisor (A1).
3. `PhotosConnector::from_config_with_geocoder(config, geocoder)` stores the
   `Option<Arc<dyn Geocoder>>`; the factory reads it off the context. `None`
   keeps the C1 coords-only fallback shape.

### Coord-dedup cache

`extract` reverse-geocodes per photo, but a coord-dedup cache avoids
re-geocoding the same spot: GPS is rounded to three decimal places (~111 m)
and scaled to an `i64` key (`GeoKey`), so an initial library scan of N photos
makes at most as many geocode calls as distinct shooting spots. Genuine
no-matches are cached (`None`); **transient network errors are not**, so a
blip does not poison a bucket for subsequent photos at the same spot. The
cache mutex is never held across an `await`. `forget` clears it.
Likewise, the staged-photo buffer mutex is held only for the in-memory
`std::mem::take` drain (no `await` while locked); the geocode loop runs after
the guard drops, so the buffer is not blocked for the ~N-second scan.

Because the geocoder already retries 429/5xx/transport failures internally
with backoff before returning `Err`, a sustained outage would otherwise
re-run that full retry sequence once *per photo* (not per spot), stalling the
whole `extract()`/sync cycle at ~1 req/s. `extract` therefore also keeps a
**per-cycle** failed-key set: once a bucket errors during one `extract()` call,
later photos in the *same* batch at that spot skip straight to the coords-only
fallback. The set is local to one cycle (not the long-lived cache), so the
next sync retries the bucket afresh — only success/no-match outcomes persist
across cycles.

<a id="place-coordinate-anchoring"></a>

## Place-coordinate anchoring (C2 / #196)

Two `entity_locations` rows are written per place fact:

- **Owner `Visited` row** (existing S3 path) — the owner was at the place,
  with the coords and the place name as its `address`, bounded by the fact's
  temporal window.
- **Place `Geographic` row** (new) — the place entity's own coordinates. A
  place does not "move", so this uses a new `LocationType::Geographic` (id 6,
  migration `046`) and the idempotent `queries::entity::ensure_place_coordinates`:
  if a `Geographic` row for the place already exists its coords are updated in
  place, otherwise a timeless row is inserted. This keeps a single row per
  place (repeated photos don't pile up move-history rows) so `find_nearby`
  (S4) can resolve places by coordinates, not only by where the owner has
  been.

The single-`Geographic`-row-per-place invariant is enforced at the schema
level by a partial unique index on `entity_id` scoped to
`location_type_id = 6` (`idx_entity_locations_geographic_unique`, migration
`047`); `ensure_place_coordinates` is a single atomic
`INSERT ... ON CONFLICT DO UPDATE` against that index. The index is partial
on purpose — `Visited`/`Home`/`Work`/`Origin`/`Current` rows are *not* unique
per `(entity_id, location_type_id)` (a person legitimately has many `Visited`
rows), so a full unique index would break them. The serial overlay worker
remains a performance optimisation, not a correctness requirement.

## Configuration (`config_json`)

```json
{
  "watch_dir": "/home/me/Pictures",
  "owner_name": "Devansh",
  "debounce_ms": 2000,
  "extensions": [".jpg", ".jpeg", ".heic"]
}
```

`watch_dir` (required, must exist) is watched recursively; `owner_name`
defaults to the slug; `debounce_ms` defaults to 2000; `extensions` defaults to
the set above. The `__slug` / `__ctype` / `__instance_id` / `__cursor` keys are
injected by the supervisor and ignored by the connector's own serde DTO.

## Public API

```rust
pub struct PhotosConnector { /* … */ }              // implements Connector (Push)
pub struct PhotosConnectorFactory;                  // implements ConnectorFactory
pub struct PhotosCursor { /* private per-file signature map */ }
impl PhotosCursor {
    pub fn from_json(Option<&str>) -> Result<Self, ConnectorError>;
    pub fn to_json(&self) -> String;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

`PhotosConnectorFactory` registers under `(ConnectorType::Photos, "local")`.

C2 additions:

```rust
pub struct ConnectorContext { pub geocoder: Option<Arc<dyn Geocoder>> }
impl PhotosConnector {
    pub fn from_config_with_geocoder(config, Option<Arc<dyn Geocoder>>) -> Result<Self, ConnectorError>;
    async fn resolve_place(Option<f64>, Option<f64>) -> Option<String>; // reverse geocode + cache
}
impl ConnectorSupervisor {
    pub fn with_geocoder(Arc<dyn Geocoder>) -> Self;
}
impl ConnectorRegistry {
    pub fn create_with_context(type, &backend, config, &ConnectorContext) -> Result<Arc<dyn Connector>, ConnectorError>;
}
```

## Testing

- Unit tests (`src/photos/`): cursor diffing/pruning/round-trip, path
  helpers, EXIF GPS+datetime parsing against committed fixtures
  (`tests/fixtures/exif.jpg`, `exif.tif`, `no_gps.jpg`, `no_exif.jpg`), fact
  conversion, config validation, signature reading, mtime fallback. Fixtures
  are generated by `tests/fixtures/gen_exif.py` (pure stdlib).
- Integration tests (`tests/photos_connector.rs`): initial scan + EXIF,
  recursive subdirs, non-image skipping, incremental skip across a simulated
  restart, changed-file reprocessing, `--full` resync, the live `notify` push
  watcher (new file + modified file), and the full supervisor →
  `KnowledgeGraph` path (fact + connector provenance + a `Visited`
  `entity_locations` row with the GPS coordinates). C2 adds: `resolve_place`
  unit tests (mock geocoder place fact, no-geocoder fallback, cache hit +
  cache-miss-then-hit, transient-error-not-cached), the `place_fact` /
  `coords_only_fact` shape tests, the geocoder `short_name` unit tests
  (`mimir-connectors/src/geocoder/`), and the integration test
  `supervisor_ingests_photo_as_took_photo_at_place_fact` (a GPS photo ingested
  through the supervisor with a mock geocoder produces a `took_photo_at`
  place fact). `mimir-knowledge/tests/normalize_test.rs` adds
  `photos_at_same_place_corroborate_and_anchor_place_coords` (two photos at
  the same place corroborate to one 0.85-confidence fact and the place's
  `Geographic` coordinates are anchored).

## Safety

The module honours the workspace `#![deny(unsafe_code)]` guarantee. The only
platform-specific code reads a file's inode via
`std::os::unix::fs::MetadataExt::ino` under `#[cfg(unix)]` — a safe accessor
that returns `0` on platforms without a stable inode.
