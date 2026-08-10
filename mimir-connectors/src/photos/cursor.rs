use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::connector::ConnectorError;

/// Stable per-file fingerprint used to skip unchanged files across restarts.
///
/// `inode` is `0` on platforms without a stable inode (non-Unix); a matching
/// `(mtime, size)` still detects modifications for those entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileSig {
    pub(super) inode: u64,
    /// mtime in milliseconds since the Unix epoch.
    pub(super) mtime_ms: i64,
    pub(super) size: u64,
}

/// Per-file signature map keyed by the path relative to the watch directory
/// (forward-slash normalised). Serialised to JSON and persisted as the
/// connector's `sync_cursor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotosCursor {
    pub(super) files: HashMap<String, FileSig>,
}

/// How a scanned file relates to the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
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
    pub(super) fn classify(&self, rel_path: &str, sig: FileSig) -> Change {
        match self.files.get(rel_path) {
            Some(prev) if *prev == sig => Change::Unchanged,
            _ => Change::NewOrChanged,
        }
    }

    /// Record/replace a file's signature.
    pub(super) fn upsert(&mut self, rel_path: String, sig: FileSig) -> bool {
        match self.files.insert(rel_path, sig) {
            None => true,
            Some(prev) => prev != sig,
        }
    }

    /// Drop entries whose paths are no longer on disk. Called during the full
    /// initial scan so the cursor tracks the live library.
    pub(super) fn prune_missing(&mut self, live: &HashMap<String, ()>) -> bool {
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
pub(super) fn file_signature(path: &Path) -> Option<FileSig> {
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
pub(super) fn relative_key(watch_dir: &Path, path: &Path) -> Option<String> {
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
pub(super) fn is_image(path: &Path, extensions: &[String]) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e,
        None => return false,
    };
    let ext = format!(".{}", ext.to_ascii_lowercase());
    extensions.contains(&ext)
}
