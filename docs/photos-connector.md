# Photos Connector (local filesystem) — `mimir-connectors::photos`

> **Phase:** 3 — Connectors (C1 / issue #195)
> **Feature flag:** `photos` (default). Framework + mock stay built without it.
> **Status:** Implemented (library only). Daemon `AppState` wiring + the
> `mimir connector …` CLI land in A1–A3 (issues #202–#204); C2 (GPS → place
> reverse-geocoding + `entity_locations` enrichment) is #196.
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

The Photos connector is the first concrete connector backend built on the
F6–F13 framework. It watches a configured local directory recursively for
image files, extracts EXIF GPS + datetime metadata with `kamadak-exif`, and
emits one `took_photo` fact per photo through the shared
`normalize_and_insert` pipeline (the supervisor owns the insert). It is
read-only, push-mode, no-network, and needs no authentication.

This is the C1 deliverable. C2 (#196) reverse-geocodes the persisted GPS
coordinates into a place name and enriches the `entity_locations` row's
`address`.

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

Each photo becomes one fact:

- **Subject** — the configured owner display name (`Person`); defaults to the
  connector slug. `owner_name` is a `config_json` field.
- **Predicate** — `took_photo` (canonicalised by the pipeline).
- **Object** — the photo's watch-dir-relative path (literal).
- **Temporal** — the EXIF `DateTimeOriginal` (with `OffsetTime*` if present,
  otherwise interpreted as UTC); falls back to the file mtime when EXIF has no
  datetime.
- **Location overlay** — when GPS is present, a `NormalizedLocation {
  location_type: Visited, address: None, latitude, longitude, timezone: None }`
  so the pipeline writes an `entity_locations` row for the owner carrying the
  raw coordinates. **C2** reverse-geocodes the `address` later.
- **Provenance** — `SourceType::Connector`, `ConnectorType::Photos`,
  `ExtractionMethod::StructuredParse` (set by the supervisor),
  `raw_reference` = the relative path.

Files with no GPS still produce a fact (no location overlay); files with no
EXIF use the file mtime. Non-image files are skipped at the extension filter
(default `.jpg .jpeg .tif .tiff .png .heif .heic .webp`; configurable). RAW
formats (CR2/ARW/NEF) are deferred (they need a dedicated raw-EXIF reader).

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

## Testing

- Unit tests (`src/photos.rs`): cursor diffing/pruning/round-trip, path
  helpers, EXIF GPS+datetime parsing against committed fixtures
  (`tests/fixtures/exif.jpg`, `exif.tif`, `no_gps.jpg`, `no_exif.jpg`), fact
  conversion, config validation, signature reading, mtime fallback. Fixtures
  are generated by `tests/fixtures/gen_exif.py` (pure stdlib).
- Integration tests (`tests/photos_connector.rs`): initial scan + EXIF,
  recursive subdirs, non-image skipping, incremental skip across a simulated
  restart, changed-file reprocessing, `--full` resync, the live `notify` push
  watcher (new file + modified file), and the full supervisor →
  `KnowledgeGraph` path (fact + connector provenance + a `Visited`
  `entity_locations` row with the GPS coordinates).

## Safety

The module honours the workspace `#![deny(unsafe_code)]` guarantee. The only
platform-specific code reads a file's inode via
`std::os::unix::fs::MetadataExt::ino` under `#[cfg(unix)]` — a safe accessor
that returns `0` on platforms without a stable inode.
