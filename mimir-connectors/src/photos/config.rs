use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default debounce window for filesystem events (~2s, per the C1 spec).
pub(super) const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(2);

/// Image extensions ingested by default (lowercase, with leading dot). Covers
/// the container formats `kamadak-exif` parses (JPEG, TIFF, HEIF, PNG, WebP);
/// RAW formats (CR2/ARW/NEF) are deferred (they need a dedicated raw-EXIF
/// reader).
pub(super) const DEFAULT_EXTENSIONS: &[&str] = &[
    ".jpg", ".jpeg", ".tif", ".tiff", ".png", ".heif", ".heic", ".webp",
];

pub(super) const DEFAULT_SLUG: &str = "photos";
pub(super) const DEFAULT_DISPLAY_NAME: &str = "Photos";

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
pub(super) struct PhotosConfigDto {
    /// Absolute path of the directory to watch recursively. Required.
    pub(super) watch_dir: String,
    /// Display name of the photo-library owner (the fact subject). Defaults
    /// to the instance slug.
    #[serde(default)]
    pub(super) owner_name: Option<String>,
    /// Debounce window in milliseconds. Defaults to 2000.
    #[serde(default = "default_debounce_ms")]
    pub(super) debounce_ms: u64,
    /// Override the ingested image extensions (lowercase, with leading dot).
    /// Defaults to [`DEFAULT_EXTENSIONS`].
    #[serde(default)]
    pub(super) extensions: Option<Vec<String>>,
}
