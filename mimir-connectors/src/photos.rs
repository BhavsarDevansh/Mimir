//! Local-filesystem Photos connector (Phase 3 C1 / issue #195).
//!
//! A read-only, no-network connector that watches a configured directory
//! recursively for image files, extracts EXIF GPS + datetime metadata with
//! [`kamadak-exif`], and emits one [`NormalizedFact`] per photo through the
//! shared `normalize_and_insert` pipeline (the supervisor owns the insert).
//! It is the first concrete connector backend built on the F6–F13 framework.
//!
//! # Ingestion model
//!
//! The connector runs in [`ConnectorMode::Push`]:
//! - The **first** [`Connector::sync`] does an initial recursive scan of the
//!   watch directory and stages every image file whose signature (inode +
//!   mtime + size) is not already in the persisted cursor. On a fresh cursor
//!   (first-ever run) this ingests the whole library; subsequent restarts
//!   skip already-processed files (the incremental cursor persisted via F8).
//! - **Subsequent** `sync` calls block on the [`notify`] debounced-event
//!   channel until filesystem events arrive, then stage only the new/changed
//!   image files. The supervisor loops immediately after a successful push
//!   cycle, so `sync` is the blocking "wait for events" point.
//!
//! [`Connector::extract`] then drains the staged raw photos into typed
//! [`NormalizedFact`]s (entity resolution, confidence, the sensitivity gate,
//! and the `entity_locations` overlay are applied by the shared pipeline).
//!
//! # Incremental cursor
//!
//! The cursor is a per-file signature map (`path -> {inode, mtime, size}`)
//! serialised to JSON and persisted by the supervisor in the `connectors`
//! `sync_cursor` column (injected back into `config_json` as `__cursor` at
//! construction, see [`crate::supervisor::ConnectorSupervisor`]). A file is
//! *unchanged* iff its path is present with a matching signature; *new* if the
//! path is absent; *changed* if the path exists but the signature differs.
//! Deleted files are pruned during the full initial scan so the cursor tracks
//! the live library. Push cycles only touch changed paths; a full
//! reconciliation (with pruning) runs on the next restart. The cursor is O(N)
//! in library size and rewritten each successful sync — acceptable for V1
//! (syncs are infrequent); a compact/dedicated cursor table is future work.
//!
//! # C1 / C2 boundary
//!
//! C1 persists the parsed GPS as a structured [`NormalizedLocation`] overlay
//! (`location_type = Visited`, `address = None`, raw `latitude`/`longitude`),
//! so `entity_locations` rows are created with coordinates immediately. C2
//! (#196) reverse-geocodes those coordinates into a place name (enriches the
//! `address`) and wires the "took a photo at `<place>`" fact. The fact
//! predicate is `took_photo` with the photo's relative path as the literal
//! object; the location overlay carries the coordinates.
//!
//! # No `unsafe`
//!
//! This module honours the workspace `#![deny(unsafe_code)]` guarantee. The
//! only platform-specific code is reading a file's inode via
//! [`std::os::unix::fs::MetadataExt::ino`] under `#[cfg(unix)]`, which is a
//! safe accessor.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc::UnboundedReceiver, mpsc::unbounded_channel};
use tracing::warn;

use crate::connector::{
    Connector, ConnectorError, ConnectorFactory, ConnectorMode, HealthStatus, SyncOptions,
    SyncOutcome,
};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType, LocationType};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::{NormalizedFact, NormalizedLocation};

use notify::{Config, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventHandler, DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt,
};

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default debounce window for filesystem events (~2s, per the C1 spec).
const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(2);

/// Image extensions ingested by default (lowercase, with leading dot). Covers
/// the container formats `kamadak-exif` parses (JPEG, TIFF, HEIF, PNG, WebP);
/// RAW formats (CR2/ARW/NEF) are deferred (they need a dedicated raw-EXIF
/// reader).
const DEFAULT_EXTENSIONS: &[&str] = &[
    ".jpg", ".jpeg", ".tif", ".tiff", ".png", ".heif", ".heic", ".webp",
];

const DEFAULT_SLUG: &str = "photos";
const DEFAULT_DISPLAY_NAME: &str = "Photos";

fn default_debounce_ms() -> u64 {
    u64::try_from(DEFAULT_DEBOUNCE.as_millis()).unwrap_or(2000)
}

// ---------------------------------------------------------------------------
// Config DTO (serde boundary for `config_json`)
// ---------------------------------------------------------------------------

/// Deserialisable configuration for [`PhotosConnector`], stored as the
/// `config_json` of a `connectors` row (with `__slug` / `__ctype` /
/// `__instance_id` / `__cursor` injected by the supervisor). Unknown fields
/// — including the injected identity/cursor keys — are ignored by serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PhotosConfigDto {
    /// Absolute path of the directory to watch recursively. Required.
    watch_dir: String,
    /// Display name of the photo-library owner (the fact subject). Defaults
    /// to the instance slug.
    #[serde(default)]
    owner_name: Option<String>,
    /// Debounce window in milliseconds. Defaults to 2000.
    #[serde(default = "default_debounce_ms")]
    debounce_ms: u64,
    /// Override the ingested image extensions (lowercase, with leading dot).
    /// Defaults to [`DEFAULT_EXTENSIONS`].
    #[serde(default)]
    extensions: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Incremental cursor
// ---------------------------------------------------------------------------

/// Stable per-file fingerprint used to skip unchanged files across restarts.
///
/// `inode` is `0` on platforms without a stable inode (non-Unix); a matching
/// `(mtime, size)` still detects modifications for those entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileSig {
    inode: u64,
    /// mtime in milliseconds since the Unix epoch.
    mtime_ms: i64,
    size: u64,
}

/// Per-file signature map keyed by the path relative to the watch directory
/// (forward-slash normalised). Serialised to JSON and persisted as the
/// connector's `sync_cursor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotosCursor {
    files: HashMap<String, FileSig>,
}

/// How a scanned file relates to the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Not in the cursor (or signatures differ) — process and record.
    NewOrChanged,
    /// Present with a matching signature — skip.
    Unchanged,
}

impl PhotosCursor {
    /// Decode a cursor from the persisted JSON string. `None` (no prior
    /// cursor, e.g. first run or after a `--full` reset) yields an empty
    /// cursor.
    pub fn from_json(cursor: Option<&str>) -> Result<Self, ConnectorError> {
        match cursor {
            None | Some("") => Ok(Self::default()),
            Some(json) => serde_json::from_str(json)
                .map_err(|error| ConnectorError::Config(format!("invalid photos cursor: {error}"))),
        }
    }

    /// Serialise the cursor for persistence.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Classify a file against the cursor.
    fn classify(&self, rel_path: &str, sig: FileSig) -> Change {
        match self.files.get(rel_path) {
            Some(prev) if *prev == sig => Change::Unchanged,
            _ => Change::NewOrChanged,
        }
    }

    /// Record/replace a file's signature.
    fn upsert(&mut self, rel_path: String, sig: FileSig) -> bool {
        match self.files.insert(rel_path, sig) {
            None => true,
            Some(prev) => prev != sig,
        }
    }

    /// Drop entries whose paths are no longer on disk. Called during the full
    /// initial scan so the cursor tracks the live library.
    fn prune_missing(&mut self, live: &HashMap<String, ()>) -> bool {
        let before = self.files.len();
        self.files.retain(|path, _| live.contains_key(path));
        self.files.len() != before
    }

    /// Number of tracked files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the cursor tracks no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Read a file's signature (inode when available, mtime, size).
///
/// Returns `None` if the metadata cannot be read (the file vanished between
/// the directory listing and the stat); callers treat that as "skip".
fn file_signature(path: &Path) -> Option<FileSig> {
    let metadata = fs::metadata(path).ok()?;
    let size = metadata.len();
    let mtime_ms = system_time_to_millis(metadata.modified().ok()?)?;
    let inode = inode_of(&metadata);
    Some(FileSig {
        inode,
        mtime_ms,
        size,
    })
}

/// Read the inode on Unix; `0` elsewhere (no stable inode).
#[cfg(unix)]
fn inode_of(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

#[cfg(not(unix))]
fn inode_of(_metadata: &fs::Metadata) -> u64 {
    0
}

fn system_time_to_millis(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).ok(),
        // Clock skewed before epoch: treat as 0 rather than failing the sync.
        Err(_) => Some(0),
    }
}

/// Normalise a path to a watch-dir-relative, forward-slash string used as the
/// cursor key and the fact's `raw_reference`. Returns `None` if `path` is not
/// under `watch_dir`.
fn relative_key(watch_dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(watch_dir).ok()?;
    let mut s = String::new();
    for (i, component) in rel.components().enumerate() {
        if i > 0 {
            s.push('/');
        }
        s.push_str(&component.as_os_str().to_string_lossy());
    }
    Some(s)
}

/// Whether `path`'s extension is an ingested image format.
fn is_image(path: &Path, extensions: &[String]) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e,
        None => return false,
    };
    let ext = format!(".{}", ext.to_ascii_lowercase());
    extensions.contains(&ext)
}

// ---------------------------------------------------------------------------
// EXIF extraction
// ---------------------------------------------------------------------------

/// Parsed EXIF fields for one image file. Missing fields are `None`; the
/// connector falls back to the file mtime for the temporal bound and emits no
/// location overlay when GPS is absent.
#[derive(Debug, Clone, PartialEq)]
struct ExifFields {
    datetime: Option<DateTime<Utc>>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

impl ExifFields {
    fn empty() -> Self {
        Self {
            datetime: None,
            latitude: None,
            longitude: None,
        }
    }
}

/// Read and parse EXIF metadata from an image file.
///
/// I/O failures (the file vanished or is unreadable) propagate as
/// [`ConnectorError::Io`]. Missing or malformed EXIF yields [`ExifFields`]
/// with all fields `None` — the caller still emits a fact using the file
/// mtime fallback and no location overlay.
fn read_exif(path: &Path) -> Result<ExifFields, ConnectorError> {
    let mut file = File::open(path)?;
    let mut reader = BufReader::new(&mut file);
    let exif = match exif::Reader::new().read_from_container(&mut reader) {
        Ok(exif) => exif,
        Err(_) => return Ok(ExifFields::empty()),
    };
    Ok(ExifFields {
        datetime: parse_exif_datetime(&exif),
        latitude: parse_exif_latitude(&exif),
        longitude: parse_exif_longitude(&exif),
    })
}

/// Parse `DateTimeOriginal` (falling back to `DateTimeDigitized` then
/// `DateTime`), applying `OffsetTimeOriginal`/`OffsetTimeDigitized`/`OffsetTime`
/// when present; otherwise the naive timestamp is interpreted as UTC.
fn parse_exif_datetime(exif: &exif::Exif) -> Option<DateTime<Utc>> {
    let field = [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ]
    .into_iter()
    .find_map(|tag| exif.get_field(tag, exif::In::PRIMARY))?;
    let raw = ascii_value(&field.value)?;
    let naive =
        NaiveDateTime::parse_from_str(raw.trim_end_matches('\0'), "%Y:%m:%d %H:%M:%S").ok()?;
    match parse_offset(exif) {
        Some(offset) => Some(
            naive
                .and_local_timezone(offset)
                .single()?
                .with_timezone(&Utc),
        ),
        None => Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)),
    }
}

fn parse_exif_latitude(exif: &exif::Exif) -> Option<f64> {
    let dms = rational3(exif, exif::Tag::GPSLatitude)?;
    let decimal = dms_to_decimal(dms);
    let negative = matches!(
        ascii_first_byte(exif, exif::Tag::GPSLatitudeRef),
        Some(b'S')
    );
    let signed = if negative { -decimal } else { decimal };
    // Reject malformed EXIF (zero-denominator rationals → `NaN`, or a corrupt
    // DMS triple outside the valid range) so garbage never reaches the
    // location overlay / proximity queries. The `took_photo` fact is still
    // emitted; only the location is dropped.
    (signed.is_finite() && (-90.0..=90.0).contains(&signed)).then_some(signed)
}

fn parse_exif_longitude(exif: &exif::Exif) -> Option<f64> {
    let dms = rational3(exif, exif::Tag::GPSLongitude)?;
    let decimal = dms_to_decimal(dms);
    let negative = matches!(
        ascii_first_byte(exif, exif::Tag::GPSLongitudeRef),
        Some(b'W')
    );
    let signed = if negative { -decimal } else { decimal };
    (signed.is_finite() && (-180.0..=180.0).contains(&signed)).then_some(signed)
}

/// Extract the first ASCII value of a field as a borrowed `&str` (NUL-trimmed
/// by the caller).
fn ascii_value(value: &exif::Value) -> Option<&str> {
    if let exif::Value::Ascii(vec) = value {
        vec.first()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    } else {
        None
    }
}

/// First byte of an ASCII-tag field (e.g. `GPSLatitudeRef` = `b'N'`/`b'S'`).
fn ascii_first_byte(exif: &exif::Exif, tag: exif::Tag) -> Option<u8> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    if let exif::Value::Ascii(vec) = &field.value {
        return vec.first().and_then(|bytes| bytes.first()).copied();
    }
    None
}

/// Three rationals (degrees, minutes, seconds) → `[f64; 3]`.
fn rational3(exif: &exif::Exif, tag: exif::Tag) -> Option<[f64; 3]> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    if let exif::Value::Rational(vec) = &field.value {
        if vec.len() == 3 {
            return Some([vec[0].to_f64(), vec[1].to_f64(), vec[2].to_f64()]);
        }
    }
    None
}

fn dms_to_decimal([deg, min, sec]: [f64; 3]) -> f64 {
    deg + min / 60.0 + sec / 3600.0
}

/// Parse an EXIF `OffsetTime*` ASCII string ("±HH:MM") into a [`FixedOffset`].
fn parse_offset(exif: &exif::Exif) -> Option<FixedOffset> {
    let field = [
        exif::Tag::OffsetTimeOriginal,
        exif::Tag::OffsetTimeDigitized,
        exif::Tag::OffsetTime,
    ]
    .into_iter()
    .find_map(|tag| exif.get_field(tag, exif::In::PRIMARY))?;
    let raw = ascii_value(&field.value)?;
    let raw = raw.trim_end_matches('\0');
    let (sign, rest) = match raw.as_bytes() {
        [b'+', ..] => (1i32, &raw[1..]),
        [b'-', ..] => (-1i32, &raw[1..]),
        _ => return None,
    };
    let (hh, mm) = rest.split_once(':')?;
    let hours: i32 = hh.parse().ok()?;
    let minutes: i32 = mm.parse().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

// ---------------------------------------------------------------------------
// Raw photo + fact conversion
// ---------------------------------------------------------------------------

/// Result of a scan/event pass: how many files were staged and whether the
/// cursor actually moved (so `sync` can report `new_cursor = None` for an
/// unchanged push cycle, matching the supervisor's nullable-cursor contract).
struct ScanResult {
    fetched: usize,
    cursor_changed: bool,
}

/// A staged raw photo awaiting extraction.
#[derive(Debug, Clone)]
struct RawPhoto {
    /// Watch-dir-relative path (cursor key + fact `raw_reference`).
    rel_path: String,
    /// EXIF `DateTimeOriginal` (with offset if present), else the file mtime.
    taken_at: DateTime<Utc>,
    /// Parsed GPS coordinates, if present.
    latitude: Option<f64>,
    longitude: Option<f64>,
}

impl RawPhoto {
    /// Build a [`NormalizedFact`] for this photo. `owner` is the subject
    /// (`Person`); the photo's relative path is the literal object; the GPS
    /// coordinates become a [`NormalizedLocation`] overlay so the shared
    /// pipeline writes an `entity_locations` row for the owner.
    fn to_fact(&self, owner: &str) -> NormalizedFact {
        NormalizedFact {
            source_type: SourceType::Connector,
            subject: owner.to_string(),
            subject_type: EntityType::Person,
            relationship_type: "took_photo".to_string(),
            object: self.rel_path.clone(),
            object_is_entity: false,
            object_type: None,
            valid_from: Some(self.taken_at),
            valid_until: None,
            is_sensitive: false,
            is_correction: false,
            correction_scope: None,
            category_ids: Vec::new(),
            recurrence: mimir_knowledge::models::enums::RecurrenceType::None,
            requires_user_action: false,
            raw_reference: Some(self.rel_path.clone()),
            location: self.location_overlay(),
        }
    }

    fn location_overlay(&self) -> Option<NormalizedLocation> {
        let (latitude, longitude) = (self.latitude?, self.longitude?);
        Some(NormalizedLocation {
            location_type: LocationType::Visited,
            address: None,
            latitude: Some(latitude),
            longitude: Some(longitude),
            timezone: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Watcher plumbing
// ---------------------------------------------------------------------------

type PhotosDebouncer = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// `DebounceEventHandler` that forwards debounced events onto a tokio
/// unbounded channel. `notify-debouncer-full`'s stable line has no tokio
/// feature, so the handler drives the channel with a synchronous `send` (the
/// debouncer runs the handler on its own thread); the connector awaits the
/// receiver inside `sync()`.
struct DebounceForwarder(tokio::sync::mpsc::UnboundedSender<DebounceEventResult>);

impl DebounceEventHandler for DebounceForwarder {
    fn handle_event(&mut self, event: DebounceEventResult) {
        // Best-effort: if the connector was dropped the receiver is gone, which
        // is a clean shutdown — swallow the send error.
        let _ = self.0.send(event);
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// Local-filesystem Photos connector (Phase 3 C1 / #195).
///
/// Push-mode, no-network, no-auth. Watches `watch_dir` recursively with a
/// debounced [`notify`] watcher, extracts EXIF GPS + datetime with
/// [`kamadak-exif`], and emits one `took_photo` fact per image with an
/// optional GPS [`NormalizedLocation`] overlay. See the module docs for the
/// ingestion model and the incremental cursor.
pub struct PhotosConnector {
    slug: String,
    display_name: String,
    watch_dir: PathBuf,
    owner_name: String,
    debounce: Duration,
    extensions: Vec<String>,
    cursor: Mutex<PhotosCursor>,
    buffer: Mutex<Vec<RawPhoto>>,
    watcher: Mutex<Option<PhotosDebouncer>>,
    events: Mutex<UnboundedReceiver<DebounceEventResult>>,
    /// `true` until the first `sync()` completes its initial scan. Drives the
    /// once-per-run full scan before the connector blocks on watch events.
    first_cycle: AtomicBool,
    started: AtomicBool,
}

impl std::fmt::Debug for PhotosConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhotosConnector")
            .field("slug", &self.slug)
            .field("watch_dir", &self.watch_dir)
            .field("owner_name", &self.owner_name)
            .field("extensions", &self.extensions)
            .finish_non_exhaustive()
    }
}

impl PhotosConnector {
    /// Build a Photos connector from its merged `config_json` value.
    ///
    /// `__slug` / `__cursor` (injected by the supervisor) are read directly;
    /// the rest is deserialised into [`PhotosConfigDto`]. Construction is
    /// cheap and synchronous: the watcher is started lazily on the first
    /// `sync()` (so a missing/unreadable directory surfaces as a sync/health
    /// error rather than a construction-time panic, and no watcher thread runs
    /// before the supervisor drives the connector).
    pub fn from_config(config: serde_json::Value) -> Result<Self, ConnectorError> {
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SLUG.to_string());

        let cursor_json = config.get("__cursor").and_then(|v| v.as_str());
        let cursor = PhotosCursor::from_json(cursor_json)?;

        let parsed: PhotosConfigDto = serde_json::from_value(config)
            .map_err(|error| ConnectorError::Config(error.to_string()))?;

        let watch_dir = PathBuf::from(&parsed.watch_dir);
        if !watch_dir.is_dir() {
            return Err(ConnectorError::Config(format!(
                "watch_dir is not an existing directory: {}",
                parsed.watch_dir
            )));
        }

        let owner_name = parsed
            .owner_name
            .filter(|name| !name.is_empty())
            .unwrap_or(slug.clone());
        let display_name = format!("{DEFAULT_DISPLAY_NAME} ({slug})");

        let extensions = parsed
            .extensions
            .map(|exts| {
                exts.into_iter()
                    .map(|e| {
                        let e = e.to_ascii_lowercase();
                        if e.starts_with('.') {
                            e
                        } else {
                            format!(".{e}")
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| DEFAULT_EXTENSIONS.iter().map(|e| e.to_string()).collect());

        // `_tx` is dropped immediately: the receiver is stored so the first
        // `sync()` can block on it until `start_watcher()` installs the real
        // debouncer-backed channel.
        let (_tx, rx) = unbounded_channel::<DebounceEventResult>();

        Ok(Self {
            slug,
            display_name,
            watch_dir,
            owner_name,
            debounce: Duration::from_millis(parsed.debounce_ms),
            extensions,
            cursor: Mutex::new(cursor),
            buffer: Mutex::new(Vec::new()),
            watcher: Mutex::new(None),
            events: Mutex::new(rx),
            first_cycle: AtomicBool::new(true),
            started: AtomicBool::new(false),
        })
    }

    /// JSON Schema describing the connector's configuration surface.
    fn config_schema_value() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["watch_dir"],
            "properties": {
                "watch_dir": {
                    "type": "string",
                    "description": "Absolute path of the directory to watch recursively for image files."
                },
                "owner_name": {
                    "type": ["string", "null"],
                    "description": "Display name of the photo-library owner (the fact subject). Defaults to the connector slug."
                },
                "debounce_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 2000,
                    "description": "Debounce window for filesystem events, in milliseconds."
                },
                "extensions": {
                    "type": ["array", "null"],
                    "items": { "type": "string" },
                    "description": "Image extensions to ingest (lowercase, with leading dot). Defaults to jpg/jpeg/tif/tiff/png/heif/heic/webp."
                }
            }
        })
    }

    /// Start the debounced watcher (idempotent). Subsequent calls are no-ops
    /// once a watcher is installed.
    ///
    /// `started` is only flipped *after* the debouncer is created and the
    /// recursive watch is registered, so a failed init (e.g. inotify watch
    /// limits, or the watch dir vanishing before the first `sync`) leaves
    /// `started == false`. The supervisor's retry then re-runs setup instead
    /// of no-op'ing and busy-looping on the construction-time (closed) event
    /// channel. The connector is driven by a single runner task, so the
    /// load/store pair is race-free.
    async fn start_watcher(&self) -> Result<(), ConnectorError> {
        if self.started.load(Ordering::SeqCst) {
            return Ok(());
        }
        let (tx, rx) = unbounded_channel::<DebounceEventResult>();
        let mut debouncer =
            new_debouncer_opt::<DebounceForwarder, notify::RecommendedWatcher, RecommendedCache>(
                self.debounce,
                None,
                DebounceForwarder(tx),
                RecommendedCache::new(),
                Config::default(),
            )
            .map_err(|error| ConnectorError::Other(format!("failed to create watcher: {error}")))?;
        debouncer
            .watch(&self.watch_dir, RecursiveMode::Recursive)
            .map_err(|error| {
                ConnectorError::Config(format!(
                    "failed to watch {}: {error}",
                    self.watch_dir.display()
                ))
            })?;
        // Fully installed before flipping the flag.
        *self.events.lock().await = rx;
        *self.watcher.lock().await = Some(debouncer);
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Recursively scan `watch_dir` for image files, staging new/changed ones
    /// (per the cursor) and pruning deleted entries. Returns the number of
    /// files staged and the live-path set (for pruning). Honours
    /// `options.full` (clears the cursor first → re-ingests everything).
    async fn initial_scan(&self, options: SyncOptions) -> Result<ScanResult, ConnectorError> {
        // Reset the cursor for a full re-ingest under a brief lock, then
        // snapshot it. The filesystem walk + per-file EXIF parse are blocking
        // I/O that can touch thousands of files, so they run off the tokio
        // worker thread via `spawn_blocking`; the cursor is written back only
        // after a successful scan (a failed scan leaves the prior cursor
        // intact so the supervisor's retry starts from the last good state).
        let mut cursor = {
            let mut guard = self.cursor.lock().await;
            if options.full {
                *guard = PhotosCursor::default();
            }
            guard.clone()
        };

        let watch_dir = self.watch_dir.clone();
        let extensions = self.extensions.clone();
        let (cursor, staged, changed) =
            tokio::task::spawn_blocking(move || -> Result<_, ConnectorError> {
                let mut live: HashMap<String, ()> = HashMap::new();
                let mut staged = Vec::new();
                let mut changed = options.full;
                scan_dir(
                    &watch_dir,
                    &watch_dir,
                    &extensions,
                    &mut |path| {
                        let Some(sig) = file_signature(path) else {
                            return;
                        };
                        let Some(rel) = relative_key(&watch_dir, path) else {
                            return;
                        };
                        live.insert(rel.clone(), ());
                        if cursor.classify(&rel, sig) == Change::NewOrChanged {
                            match stage_file(path, &rel, sig) {
                                Ok(raw) => {
                                    changed |= cursor.upsert(rel, sig);
                                    staged.push(raw);
                                }
                                Err(error) => {
                                    // A single unreadable/unparseable file must
                                    // not abort the scan; record its signature so
                                    // it is not retried every cycle, and log.
                                    warn!(path = %path.display(), error = %error, "skipping photo file");
                                    changed |= cursor.upsert(rel, sig);
                                }
                            }
                        } else {
                            changed |= cursor.upsert(rel, sig);
                        }
                    },
                )?;
                changed |= cursor.prune_missing(&live);
                Ok((cursor, staged, changed))
            })
            .await
            .map_err(|join| ConnectorError::Other(format!("photo scan task failed: {join}")))??;
        let count = staged.len();
        *self.cursor.lock().await = cursor;
        self.buffer.lock().await.extend(staged);
        Ok(ScanResult {
            fetched: count,
            cursor_changed: changed,
        })
    }

    /// Process one debounced event batch: collect changed image paths, stage
    /// new/changed ones, and update the cursor.
    async fn process_events(
        &self,
        events: &DebounceEventResult,
    ) -> Result<ScanResult, ConnectorError> {
        let mut cursor = self.cursor.lock().await;
        let mut changed = false;
        let mut staged = Vec::new();
        let event_paths = match events {
            Ok(events) => events
                .iter()
                .flat_map(|e| e.event.paths.clone())
                .collect::<Vec<_>>(),
            Err(errors) => {
                for error in errors {
                    warn!(error = %error, "watcher error");
                }
                // A transient watcher error is not a cursor change; report an
                // empty, unchanged result so the supervisor only touches
                // `last_sync_at`.
                return Ok(ScanResult {
                    fetched: 0,
                    cursor_changed: false,
                });
            }
        };

        for path in event_paths {
            if !is_image(&path, &self.extensions) {
                continue;
            }
            // Create/modify → record; remove → drop the cursor entry. Any
            // other kind (access/other) is ignored to avoid reprocessing.
            let metadata = fs::metadata(&path).ok();
            match (metadata, path.exists()) {
                (Some(_), _) => {
                    let Some(sig) = file_signature(&path) else {
                        continue;
                    };
                    let Some(rel) = relative_key(&self.watch_dir, &path) else {
                        continue;
                    };
                    if cursor.classify(&rel, sig) == Change::NewOrChanged {
                        match stage_file(&path, &rel, sig) {
                            Ok(raw) => {
                                changed |= cursor.upsert(rel, sig);
                                staged.push(raw);
                            }
                            Err(error) => {
                                warn!(path = %path.display(), error = %error, "skipping photo file");
                                changed |= cursor.upsert(rel, sig);
                            }
                        }
                    } else {
                        changed |= cursor.upsert(rel, sig);
                    }
                }
                (_, false) => {
                    if let Some(rel) = relative_key(&self.watch_dir, &path) {
                        changed |= cursor.files.remove(&rel).is_some();
                    }
                }
                _ => {}
            }
        }

        let count = staged.len();
        self.buffer.lock().await.extend(staged);
        Ok(ScanResult {
            fetched: count,
            cursor_changed: changed,
        })
    }
}

/// Stage a single file: read its EXIF and build a [`RawPhoto`]. The temporal
/// bound is the EXIF `DateTimeOriginal` when present, else the file mtime
/// carried in `sig` (already computed by the scan — no second stat).
fn stage_file(path: &Path, rel_path: &str, sig: FileSig) -> Result<RawPhoto, ConnectorError> {
    let exif = read_exif(path)?;
    let taken_at = exif
        .datetime
        .or_else(|| DateTime::<Utc>::from_timestamp_millis(sig.mtime_ms))
        .unwrap_or_else(Utc::now);
    Ok(RawPhoto {
        rel_path: rel_path.to_string(),
        taken_at,
        latitude: exif.latitude,
        longitude: exif.longitude,
    })
}

/// Recursively visit files under `root`, calling `visit` for each entry.
/// Symlinks are not followed (avoids cycles); per-entry errors are logged and
/// skipped so one unreadable subtree does not abort the scan.
fn scan_dir(
    root: &Path,
    dir: &Path,
    extensions: &[String],
    visit: &mut impl FnMut(&Path),
) -> Result<(), ConnectorError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            // A missing subdir (vanished mid-scan) is not fatal to the root.
            if dir != root {
                warn!(dir = %dir.display(), error = %error, "skipping unreadable directory");
                return Ok(());
            }
            return Err(ConnectorError::Io(error));
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!(error = %error, "skipping unreadable directory entry");
                continue;
            }
        };
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(meta) => meta,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "skipping unreadable entry");
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "skipping unreadable entry");
                continue;
            }
        };
        if file_type.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            scan_dir(root, &path, extensions, visit)?;
        } else if metadata.is_file() && is_image(&path, extensions) {
            visit(&path);
        }
    }
    Ok(())
}

#[async_trait]
impl Connector for PhotosConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Photos
    }

    fn mode(&self) -> ConnectorMode {
        ConnectorMode::Push
    }

    fn config_schema(&self) -> serde_json::Value {
        Self::config_schema_value()
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        // No credentials: a local-FS connector is always "authenticated" as
        // long as the watch directory still exists.
        if self.watch_dir.is_dir() {
            Ok(ConnectorAuthState::Authenticated)
        } else {
            Ok(ConnectorAuthState::Unauthenticated)
        }
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        if self.watch_dir.is_dir() {
            Ok(HealthStatus::Online)
        } else {
            Ok(HealthStatus::Offline)
        }
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        self.start_watcher().await?;

        // The first cycle runs an initial recursive scan (catching changes
        // made while the daemon was down) before the connector starts blocking
        // on watch events. The flag is only consumed on success: if the scan
        // fails (e.g. the watch root became unreadable between construction
        // and the first cycle) it is restored so the supervisor's retry
        // re-runs the scan instead of skipping it and missing every
        // pre-existing file until a restart.
        if self.first_cycle.swap(false, Ordering::SeqCst) {
            match self.initial_scan(options).await {
                Ok(result) => return Ok(self.outcome(result).await),
                Err(error) => {
                    self.first_cycle.store(true, Ordering::SeqCst);
                    return Err(error);
                }
            }
        }

        // Push wait: block for the next debounced event batch, then process it.
        let event = self.events.lock().await.recv().await;
        let result = match event {
            Some(events) => self.process_events(&events).await?,
            None => {
                // The debouncer's sender is gone (its thread stopped/panicked)
                // while `started` is still `true`. Returning `Ok` here would
                // make `sync` complete instantly and the supervisor would
                // re-call it in a tight 100%-CPU loop. Surface it as a sync
                // failure so the supervisor's backoff + circuit breaker engage
                // instead of hot-spinning.
                return Err(ConnectorError::Other(
                    "photos watcher event channel closed unexpectedly".to_string(),
                ));
            }
        };
        Ok(self.outcome(result).await)
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        let mut buffer = self.buffer.lock().await;
        let owner = self.owner_name.clone();
        let facts: Vec<NormalizedFact> = std::mem::take(&mut *buffer)
            .into_iter()
            .map(|raw| raw.to_fact(&owner))
            .collect();
        Ok(facts)
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        // Drop the in-memory cursor and buffer; persisted KB facts are
        // cascaded by the supervisor via the trash machinery. Reset the
        // lifecycle flags so a connector reused after `forget` re-runs its
        // initial scan and restarts its watcher.
        *self.cursor.lock().await = PhotosCursor::default();
        self.buffer.lock().await.clear();
        if let Some(watcher) = self.watcher.lock().await.take() {
            watcher.stop();
        }
        self.started.store(false, Ordering::SeqCst);
        self.first_cycle.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl PhotosConnector {
    /// Build a [`SyncOutcome`] for `fetched` files, advancing the persisted
    /// cursor.
    async fn outcome(&self, result: ScanResult) -> SyncOutcome {
        // Only report the cursor when it actually moved; an unchanged push
        // cycle returns `new_cursor = None` so the supervisor just stamps
        // `last_sync_at` without rewriting the progress token.
        let new_cursor = if result.cursor_changed {
            Some(self.cursor.lock().await.to_json())
        } else {
            None
        };
        SyncOutcome {
            fetched: u32::try_from(result.fetched).unwrap_or(u32::MAX),
            new_cursor,
            fetched_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// [`ConnectorFactory`] that builds a [`PhotosConnector`] from its
/// `config_json`. Gated behind the `photos` feature.
#[derive(Debug, Default)]
pub struct PhotosConnectorFactory;

impl ConnectorFactory for PhotosConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError> {
        let connector = PhotosConnector::from_config(config)?;
        Ok(std::sync::Arc::new(connector) as std::sync::Arc<dyn Connector>)
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure pieces: cursor diffing, EXIF parsing, path
    //! helpers, and fact conversion. Watcher/supervisor integration lives in
    //! `tests/photos_connector.rs`.

    use super::*;
    use std::fs;

    fn fixture(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures");
        p.push(name);
        p
    }

    // -- cursor --

    #[test]
    fn cursor_round_trips_json() {
        let mut cursor = PhotosCursor::default();
        cursor.upsert(
            "a/b.jpg".to_string(),
            FileSig {
                inode: 42,
                mtime_ms: 1_700_000_000_000,
                size: 1024,
            },
        );
        let json = cursor.to_json();
        let back = PhotosCursor::from_json(Some(&json)).unwrap();
        assert_eq!(cursor, back);
    }

    #[test]
    fn cursor_classifies_new_changed_unchanged() {
        let mut cursor = PhotosCursor::default();
        let sig = FileSig {
            inode: 1,
            mtime_ms: 100,
            size: 10,
        };
        cursor.upsert("x.jpg".to_string(), sig);

        assert_eq!(cursor.classify("x.jpg", sig), Change::Unchanged);
        assert_eq!(
            cursor.classify(
                "x.jpg",
                FileSig {
                    mtime_ms: 200,
                    ..sig
                }
            ),
            Change::NewOrChanged
        );
        assert_eq!(cursor.classify("y.jpg", sig), Change::NewOrChanged);
    }

    #[test]
    fn cursor_prunes_missing() {
        let mut cursor = PhotosCursor::default();
        cursor.upsert(
            "gone.jpg".to_string(),
            FileSig {
                inode: 1,
                mtime_ms: 1,
                size: 1,
            },
        );
        cursor.upsert(
            "kept.jpg".to_string(),
            FileSig {
                inode: 2,
                mtime_ms: 2,
                size: 2,
            },
        );
        let live = [("kept.jpg".to_string(), ())].into_iter().collect();
        cursor.prune_missing(&live);
        assert!(cursor.files.contains_key("kept.jpg"));
        assert!(!cursor.files.contains_key("gone.jpg"));
    }

    #[test]
    fn cursor_none_yields_empty() {
        assert!(PhotosCursor::from_json(None).unwrap().is_empty());
        assert!(PhotosCursor::from_json(Some("")).unwrap().is_empty());
    }

    // -- path helpers --

    #[test]
    fn relative_key_normalises_to_forward_slash() {
        let root = Path::new("/tmp/photos");
        let key = relative_key(root, Path::new("/tmp/photos/sub/IMG_001.jpg")).unwrap();
        assert_eq!(key, "sub/IMG_001.jpg");
    }

    #[test]
    fn is_image_matches_default_extensions_case_insensitively() {
        let exts: Vec<String> = DEFAULT_EXTENSIONS.iter().map(|e| e.to_string()).collect();
        assert!(is_image(Path::new("photo.JPG"), &exts));
        assert!(is_image(Path::new("photo.tiff"), &exts));
        assert!(!is_image(Path::new("photo.txt"), &exts));
        assert!(!is_image(Path::new("noext"), &exts));
    }

    // -- EXIF parsing against committed fixtures --

    #[test]
    fn parses_jpeg_exif_gps_and_datetime() {
        let fields = read_exif(&fixture("exif.jpg")).unwrap();
        let datetime = fields.datetime.expect("datetime");
        assert_eq!(
            datetime.format("%Y:%m:%d %H:%M:%S").to_string(),
            "2024:05:15 14:30:00"
        );
        // No OffsetTime in the fixture → interpreted as UTC.
        assert_eq!(
            datetime,
            DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDateTime::parse_from_str("2024:05:15 14:30:00", "%Y:%m:%d %H:%M:%S").unwrap(),
                Utc,
            )
        );
        let lat = fields.latitude.expect("latitude");
        let lon = fields.longitude.expect("longitude");
        assert!((lat - 46.5).abs() < 1e-6, "latitude {lat}");
        assert!((lon - 7.5).abs() < 1e-6, "longitude {lon}");
    }

    #[test]
    fn parses_tiff_exif_gps_and_datetime() {
        let fields = read_exif(&fixture("exif.tif")).unwrap();
        assert!(fields.datetime.is_some());
        assert!((fields.latitude.unwrap() - 46.5).abs() < 1e-6);
        assert!((fields.longitude.unwrap() - 7.5).abs() < 1e-6);
    }

    #[test]
    fn no_gps_yields_no_location() {
        let fields = read_exif(&fixture("no_gps.jpg")).unwrap();
        assert!(fields.datetime.is_some());
        assert!(fields.latitude.is_none());
        assert!(fields.longitude.is_none());
    }

    #[test]
    fn no_exif_yields_empty_fields() {
        let fields = read_exif(&fixture("no_exif.jpg")).unwrap();
        assert!(fields.datetime.is_none());
        assert!(fields.latitude.is_none());
        assert!(fields.longitude.is_none());
    }

    // -- fact conversion --

    #[test]
    fn raw_photo_with_gps_emits_location_overlay() {
        let raw = RawPhoto {
            rel_path: "2024/IMG_001.jpg".to_string(),
            taken_at: DateTime::<Utc>::from_timestamp(1_715_000_000, 0).unwrap(),
            latitude: Some(46.5),
            longitude: Some(7.5),
        };
        let fact = raw.to_fact("Devansh");
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
    fn raw_photo_without_gps_has_no_location_overlay() {
        let raw = RawPhoto {
            rel_path: "no_gps.jpg".to_string(),
            taken_at: DateTime::<Utc>::from_timestamp(1_715_000_000, 0).unwrap(),
            latitude: None,
            longitude: None,
        };
        let fact = raw.to_fact("Devansh");
        assert!(fact.location.is_none());
    }

    // -- config --

    #[test]
    fn config_requires_existing_watch_dir() {
        let config = serde_json::json!({ "watch_dir": "/definitely/not/here/xyz" });
        assert!(PhotosConnector::from_config(config).is_err());
    }

    #[test]
    fn config_loads_seeded_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut cursor = PhotosCursor::default();
        cursor.upsert(
            "seen.jpg".to_string(),
            FileSig {
                inode: 9,
                mtime_ms: 5,
                size: 5,
            },
        );
        let cursor_json = cursor.to_json();
        let config = serde_json::json!({
            "watch_dir": dir.path().to_string_lossy(),
            "__cursor": cursor_json,
        });
        let connector = PhotosConnector::from_config(config).unwrap();
        assert_eq!(connector.cursor.try_lock().unwrap().len(), 1);
    }

    #[test]
    fn config_uses_slug_when_owner_absent() {
        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::json!({
            "watch_dir": dir.path().to_string_lossy(),
            "__slug": "my-photos",
        });
        let connector = PhotosConnector::from_config(config).unwrap();
        assert_eq!(connector.owner_name, "my-photos");
        assert_eq!(connector.id(), "my-photos");
    }

    // -- signature --

    #[test]
    fn file_signature_reads_inode_mtime_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jpg");
        fs::write(&path, b"hello").unwrap();
        let sig = file_signature(&path).unwrap();
        assert_eq!(sig.size, 5);
        assert!(sig.mtime_ms > 0);
        // On Unix the inode is non-zero; elsewhere it is 0.
        #[cfg(unix)]
        assert_ne!(sig.inode, 0);
    }

    #[test]
    fn stage_file_falls_back_to_mtime_without_exif() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bare.jpg");
        fs::write(&path, b"not really a jpeg").unwrap();
        let sig = file_signature(&path).expect("signature");
        let raw = stage_file(&path, "bare.jpg", sig).unwrap();
        assert!(raw.latitude.is_none());
        assert!(raw.longitude.is_none());
        // taken_at falls back to the file mtime carried in the signature.
        let expected = DateTime::<Utc>::from_timestamp_millis(sig.mtime_ms).unwrap();
        assert_eq!(raw.taken_at, expected);
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
}
