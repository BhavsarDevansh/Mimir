use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};

use tracing::warn;

use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{LocationType, RecurrenceType};
use mimir_knowledge::normalize::{NormalizedFact, NormalizedLocation};

use crate::connector::ConnectorError;
use crate::fact::connector_fact;
use crate::photos::connector::geo_key;
use crate::photos::cursor::{FileSig, is_image};
use crate::photos::exif::read_exif;

/// Result of a scan/event pass: how many files were staged and whether the
/// cursor actually moved (so `sync` can report `new_cursor = None` for an
/// unchanged push cycle, matching the supervisor's nullable-cursor contract).
pub(super) struct ScanResult {
    pub(super) fetched: usize,
    pub(super) cursor_changed: bool,
}

/// A staged raw photo awaiting extraction.
#[derive(Debug, Clone)]
pub(super) struct RawPhoto {
    /// Watch-dir-relative path (cursor key + fact `raw_reference`).
    pub(super) rel_path: String,
    /// EXIF `DateTimeOriginal` (with offset if present), else the file mtime.
    pub(super) taken_at: DateTime<Utc>,
    /// Parsed GPS coordinates, if present.
    pub(super) latitude: Option<f64>,
    pub(super) longitude: Option<f64>,
}

impl RawPhoto {
    /// Build a [`NormalizedFact`] for this photo (Phase 3 C2 / #196).
    ///
    /// `owner` is the subject (`Person`): the canonical user identity when
    /// injected, else the configured owner name (issue #246). When `place`
    /// is the resolved locality name, the fact is `owner took_photo_at
    /// <place>` with the place as a `Place` object entity and a
    /// [`NormalizedLocation`] overlay carrying the coords *and* the place
    /// name as its address — the shared pipeline writes a `Visited`
    /// `entity_locations` row for the owner and anchors the place entity's
    /// coordinates. When the photo has GPS but no place name resolves (no
    /// geocoder, no match, or a transient geocode error), the fact expresses
    /// the real-world event as `owner visited <coords-label>` with the
    /// coords-only overlay (issue #250) — the photo file is provenance, never
    /// the fact's object. A photo without GPS keeps the literal
    /// `took_photo <rel_path>` record (no location data exists to express a
    /// visit). In all cases `raw_reference` is the photo's relative path
    /// (the native source id) and `valid_from` is the EXIF timestamp.
    pub(super) fn to_fact(&self, owner: &str, place: Option<String>) -> NormalizedFact {
        match place {
            Some(name) => self.place_fact(owner, name),
            None => match (self.latitude, self.longitude) {
                (Some(lat), Some(lng)) => self.visited_fact(owner, geo_key(lat, lng).label()),
                _ => self.took_photo_fact(owner),
            },
        }
    }

    /// The C2 "took a photo at `<place>`" fact: the place is a `Place` object
    /// entity, and the location overlay carries coords + the place name.
    pub(super) fn place_fact(&self, owner: &str, place: String) -> NormalizedFact {
        connector_fact(
            owner.to_string(),
            EntityType::Person,
            "took_photo_at",
            place.clone(),
            true,
            Some(EntityType::Place),
            Some(self.taken_at),
            None,
            RecurrenceType::None,
            &self.rel_path,
            None,
            None,
            self.location_overlay_with_address(place),
        )
    }

    /// The coords-only fallback (issue #250): an `owner visited <label>`
    /// fact with a coords-only location overlay. Used when a photo has GPS
    /// but no place name resolves (no geocoder, no match, or a transient
    /// geocode error), so the photo's GPS is never dropped. The object is a
    /// stable millidegree label for the photo's GPS bucket (the same
    /// rounding as the reverse-geocode cache key), so photos at the same
    /// ~111 m spot author the same object and corroborate into one fact per
    /// spot. The photo path stays as `raw_reference` provenance.
    pub(super) fn visited_fact(&self, owner: &str, label: String) -> NormalizedFact {
        connector_fact(
            owner.to_string(),
            EntityType::Person,
            "visited",
            label,
            false,
            None,
            Some(self.taken_at),
            None,
            RecurrenceType::None,
            &self.rel_path,
            None,
            None,
            self.coords_only_overlay(),
        )
    }

    /// The no-GPS record: an `owner took_photo <rel_path>` literal-object
    /// fact with no location overlay. A photo without GPS evidences no
    /// real-world visit, so the literal photo record (timestamp only) is
    /// kept for "how many photos did I take" queries; the path is also the
    /// `raw_reference` provenance.
    pub(super) fn took_photo_fact(&self, owner: &str) -> NormalizedFact {
        connector_fact(
            owner.to_string(),
            EntityType::Person,
            "took_photo",
            self.rel_path.clone(),
            false,
            None,
            Some(self.taken_at),
            None,
            RecurrenceType::None,
            &self.rel_path,
            None,
            None,
            None,
        )
    }

    /// Location overlay for the place fact: coords + the resolved place name
    /// as the address (the shared pipeline upserts a `Visited` row for the
    /// owner with both halves already filled).
    pub(super) fn location_overlay_with_address(
        &self,
        place: String,
    ) -> Option<NormalizedLocation> {
        let (latitude, longitude) = (self.latitude?, self.longitude?);
        Some(NormalizedLocation {
            location_type: LocationType::Visited,
            address: Some(place),
            latitude: Some(latitude),
            longitude: Some(longitude),
            timezone: None,
        })
    }

    /// Coords-only location overlay (coords-only fallback, issue #250).
    pub(super) fn coords_only_overlay(&self) -> Option<NormalizedLocation> {
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

/// Stage a single file: read its EXIF and build a [`RawPhoto`]. The temporal
/// bound is the EXIF `DateTimeOriginal` when present, else the file mtime
/// carried in `sig` (already computed by the scan — no second stat).
pub(super) fn stage_file(
    path: &Path,
    rel_path: &str,
    sig: FileSig,
) -> Result<RawPhoto, ConnectorError> {
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
pub(super) fn scan_dir(
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
