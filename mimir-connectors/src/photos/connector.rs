use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use mimir_core::geocoder::Geocoder;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

use crate::connector::{
    Connector, ConnectorError, ConnectorMode, HealthStatus, SyncOptions, SyncOutcome,
};
use crate::photos::config::{
    DEFAULT_DISPLAY_NAME, DEFAULT_EXTENSIONS, DEFAULT_SLUG, PhotosConfigDto,
};
use crate::photos::cursor::PhotosCursor;
use crate::photos::scan::{RawPhoto, ScanResult};
use crate::photos::watcher::PhotosDebouncer;
use notify_debouncer_full::DebounceEventResult;

/// Local-filesystem Photos connector (Phase 3 C1 / #195).
///
/// Push-mode, no-network, no-auth. Watches `watch_dir` recursively with a
/// debounced `notify` watcher, extracts EXIF GPS + datetime with
/// `kamadak-exif`, and emits one fact per image — `took_photo_at <place>`
/// when GPS resolves a place name, `visited <coords-label>` when GPS has no
/// place name (issue #250), or `took_photo <rel_path>` when there is no GPS —
/// each with an optional GPS
/// [`NormalizedLocation`](mimir_knowledge::normalize::NormalizedLocation)
/// overlay. See the module docs for the ingestion model and the incremental
/// cursor.
pub struct PhotosConnector {
    pub(super) slug: String,
    pub(super) display_name: String,
    pub(super) watch_dir: PathBuf,
    /// Per-instance owner display name (the fact-subject fallback). Used
    /// only when no canonical user identity is injected (issue #246);
    /// defaults to the connector slug.
    pub(super) owner_name: String,
    /// Canonical user identity name (the `config.toml` `[identity] name`),
    /// injected via [`ConnectorContext::user_identity`](crate::connector::ConnectorContext::user_identity)
    /// (issue #246). When present, photo facts are authored against this
    /// identity so they resolve to the same `Person` entity the daemon
    /// resolves as `user_entity_id` (and surface in user-scoped memory
    /// sections). When `None`, [`owner_name`](Self::owner_name) is the
    /// subject fallback.
    pub(super) user_identity: Option<String>,
    pub(super) debounce: Duration,
    pub(super) extensions: Vec<String>,
    pub(super) cursor: Mutex<PhotosCursor>,
    pub(super) buffer: Mutex<Vec<RawPhoto>>,
    pub(super) watcher: Mutex<Option<PhotosDebouncer>>,
    pub(super) events: Mutex<UnboundedReceiver<DebounceEventResult>>,
    /// Shared geocoder used to reverse-geocode EXIF GPS into a place name
    /// during `extract` (Phase 3 C2 / #196). `None` when no geocoder is
    /// configured; photos with GPS then fall back to the coords-only
    /// `visited <coords-label>` shape (issue #250) so no data is lost.
    geocoder: Option<std::sync::Arc<dyn Geocoder>>,
    /// Coord-dedup cache for reverse geocoding (Phase 3 C2 / #196): rounded
    /// GPS → resolved place short name. Bounds Nominatim calls to one per
    /// ~100 m bucket rather than one per photo, so an initial library scan of
    /// N photos makes at most as many geocode calls as distinct shooting
    /// spots. `None` entries (a genuine no-match) are cached; transient
    /// network errors are not, so a blip does not poison the bucket.
    pub(super) geocode_cache: Mutex<HashMap<GeoKey, Option<String>>>,
    /// `true` until the first `sync()` completes its initial scan. Drives the
    /// once-per-run full scan before the connector blocks on watch events.
    pub(super) first_cycle: AtomicBool,
    pub(super) started: AtomicBool,
}

/// Rounded coordinate key for the reverse-geocode cache. Three decimal
/// places (~111 m at the equator) group photos taken at the same spot while
/// keeping distinct neighbourhoods separate. Stored as scaled integers so the
/// map is `Eq`/`Hash` (raw `f64` is not).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GeoKey(i64, i64);

/// Round a coordinate to three decimal places and scale to an integer key.
pub(super) fn geo_key(latitude: f64, longitude: f64) -> GeoKey {
    GeoKey(
        (latitude * 1000.0).round() as i64,
        (longitude * 1000.0).round() as i64,
    )
}

impl std::fmt::Debug for PhotosConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhotosConnector")
            .field("slug", &self.slug)
            .field("watch_dir", &self.watch_dir)
            .field("owner_name", &self.owner_name)
            .field("user_identity", &self.user_identity)
            .field("extensions", &self.extensions)
            .finish_non_exhaustive()
    }
}

impl PhotosConnector {
    /// Build a Photos connector from its merged `config_json` value.
    ///
    /// `__slug` / `__cursor` (injected by the supervisor) are read directly;
    /// the rest is deserialised into `PhotosConfigDto`. Construction is
    /// cheap and synchronous: the watcher is started lazily on the first
    /// `sync()` (so a missing/unreadable directory surfaces as a sync/health
    /// error rather than a construction-time panic, and no watcher thread runs
    /// before the supervisor drives the connector).
    pub fn from_config(config: serde_json::Value) -> Result<Self, ConnectorError> {
        Self::from_config_with_geocoder(config, None, None)
    }

    /// Build a Photos connector from its merged `config_json` value plus a
    /// shared geocoder injected via the [`ConnectorContext`](crate::connector::ConnectorContext)
    /// (Phase 3 C2 / #196) and the canonical user identity (issue #246).
    ///
    /// The geocoder is used in `extract` to reverse-geocode EXIF GPS into a
    /// locality-level place name that becomes the object of a
    /// `took_photo_at` fact. `None` keeps the coords-only `visited` fallback
    /// shape (issue #250). `user_identity` is the canonical `[identity] name`
    /// (mirroring the Calendar connector): when present it is the subject of
    /// every photo fact; when `None` the per-instance `owner_name` (defaulting
    /// to the slug) is used instead. Construction stays cheap and
    /// synchronous; the watcher is started lazily on the first `sync()`.
    pub fn from_config_with_geocoder(
        config: serde_json::Value,
        geocoder: Option<std::sync::Arc<dyn Geocoder>>,
        user_identity: Option<String>,
    ) -> Result<Self, ConnectorError> {
        // Normalise like the Calendar/Email connectors (trim, blank → None)
        // so a padded identity can never author facts against a `Person`
        // entity that differs from the canonical trimmed one.
        let user_identity = crate::connector::normalize_user_identity(user_identity);
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

        // Normalise like the identity (trim, blank → None) so the documented
        // "defaults to the connector slug" holds even for a whitespace-only
        // `owner_name`, and no fact is ever authored against a blank subject.
        let owner_name =
            crate::connector::normalize_user_identity(parsed.owner_name).unwrap_or(slug.clone());
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
            user_identity,
            debounce: Duration::from_millis(parsed.debounce_ms),
            extensions,
            cursor: Mutex::new(cursor),
            buffer: Mutex::new(Vec::new()),
            watcher: Mutex::new(None),
            events: Mutex::new(rx),
            geocoder,
            geocode_cache: Mutex::new(HashMap::new()),
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
                    "description": "Display name of the photo-library owner (the fact subject). Used only when no canonical user identity ([identity] name) is injected; defaults to the connector slug."
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
        // The canonical user identity wins when injected (issue #246); the
        // per-instance `owner_name` (defaulting to the slug) is the fallback
        // so a library without a configured `[identity] name` still produces
        // facts.
        let owner = self.user_identity.as_deref().unwrap_or(&self.owner_name);
        // Drain the buffer into a local Vec and drop the guard *before* the
        // per-photo reverse-geocode loop. `resolve_place` awaits
        // `geocoder.reverse()` — a rate-limited (~1 req/s for Nominatim)
        // network call — once per distinct shooting spot, so holding the
        // buffer mutex across it would block `forget`/`reset` or a concurrent
        // admin-triggered sync for the full scan duration (~N seconds). The
        // `std::mem::take` swap replaces the buffer's contents in place while
        // we still hold the lock, and the guard is released at the end of the
        // block, restoring the original hold-time (in-memory map only).
        let raws = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };
        // Per-extract() set of GPS buckets that already errored this cycle.
        // The geocoder retries 429/5xx/transport failures internally with
        // backoff before returning `Err`, so without this guard a sustained
        // outage would re-run the full retry sequence for *every photo*
        // (not every distinct spot), stalling the whole sync at ~1 req/s.
        // The set is local to one `extract()` call, so the next sync cycle
        // retries the bucket — only the long-lived success/no-match cache
        // (`geocode_cache`) persists across cycles.
        let mut failed_this_cycle: HashSet<GeoKey> = HashSet::new();
        let mut facts = Vec::with_capacity(raws.len());
        for raw in raws {
            // Reverse-geocode each photo's GPS into a place name (with the
            // coord-dedup cache) before building the fact. Geocode failures
            // are tolerated per-photo — a photo whose GPS cannot be resolved
            // degrades to the coords-only `visited` shape (issue #250)
            // rather than failing the whole extraction.
            let place = self
                .resolve_place(raw.latitude, raw.longitude, &mut failed_this_cycle)
                .await;
            facts.push(raw.to_fact(owner, place));
        }
        Ok(facts)
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        // Drop the in-memory cursor and buffer; persisted KB facts are
        // cascaded by the supervisor via the trash machinery. Reset the
        // lifecycle flags so a connector reused after `forget` re-runs its
        // initial scan and restarts its watcher.
        *self.cursor.lock().await = PhotosCursor::default();
        self.buffer.lock().await.clear();
        self.geocode_cache.lock().await.clear();
        if let Some(watcher) = self.watcher.lock().await.take() {
            watcher.stop();
        }
        self.started.store(false, Ordering::SeqCst);
        self.first_cycle.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl PhotosConnector {
    /// Resolve a photo's GPS to a locality-level place short name (Phase 3 C2
    /// / #196), using the coord-dedup cache to avoid re-geocoding the same
    /// spot. Returns `None` when there is no geocoder, no GPS, no match, or a
    /// transient geocode error — the caller then builds the coords-only
    /// `visited` fallback fact (issue #250).
    async fn resolve_place(
        &self,
        latitude: Option<f64>,
        longitude: Option<f64>,
        failed_this_cycle: &mut HashSet<GeoKey>,
    ) -> Option<String> {
        let geocoder = self.geocoder.clone()?;
        let (lat, lng) = (latitude?, longitude?);
        let key = geo_key(lat, lng);

        // Skip a bucket that already errored during this `extract()` cycle so
        // a sustained outage degrades quickly to the coords-only fallback
        // instead of re-running the geocoder's internal retry backoff once
        // per photo. The set is per-cycle, so the next sync retries afresh.
        if failed_this_cycle.contains(&key) {
            return None;
        }

        // Cache hit: clone the cached value out and release the lock before
        // any await (never hold the cache mutex across the geocode call).
        if let Some(cached) = self.geocode_cache.lock().await.get(&key).cloned() {
            return cached;
        }

        // Cache miss: reverse-geocode without holding the lock. Genuine
        // no-matches are cached; transient errors are not (so a network blip
        // does not poison the bucket for subsequent photos at the same spot)
        // — but the per-cycle `failed_this_cycle` set still bounds a sustained
        // outage to one attempt per spot per `extract()` call.
        let (value, cacheable) = match geocoder.reverse(lat, lng).await {
            Ok(Some(result)) => (result.short_name, true),
            Ok(None) => {
                tracing::debug!("no place found for photo GPS ({lat}, {lng})");
                (None, true)
            }
            Err(error) => {
                tracing::warn!("reverse geocode failed for photo GPS ({lat}, {lng}): {error}");
                failed_this_cycle.insert(key);
                (None, false)
            }
        };
        if cacheable {
            self.geocode_cache.lock().await.insert(key, value.clone());
        }
        value
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

#[cfg(test)]
#[path = "behaviour_tests.rs"]
mod behaviour_tests;
#[cfg(test)]
#[path = "logic_tests.rs"]
mod logic_tests;
