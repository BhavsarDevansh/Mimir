use std::collections::HashMap;
use std::fs;

use notify_debouncer_full::DebounceEventResult;
use tracing::warn;

use crate::connector::{ConnectorError, SyncOptions};
use crate::photos::connector::PhotosConnector;
use crate::photos::cursor::{Change, PhotosCursor, file_signature, is_image, relative_key};
use crate::photos::scan::{ScanResult, scan_dir, stage_file};

impl PhotosConnector {
    /// Recursively scan `watch_dir` for image files, staging new/changed ones
    /// (per the cursor) and pruning deleted entries. Returns the number of
    /// files staged and the live-path set (for pruning). Honours
    /// `options.full` (clears the cursor first → re-ingests everything).
    pub(super) async fn initial_scan(
        &self,
        options: SyncOptions,
    ) -> Result<ScanResult, ConnectorError> {
        // Reset the cursor for a full re-ingest under a brief lock, then
        // snapshot it. The filesystem walk + per-file EXIF parse are blocking
        // I/O that can touch thousands of files, so they run off the tokio
        // worker thread via `spawn_blocking`. The computed cursor is returned
        // (not written back): the supervisor persists it and hands it back
        // via `on_cycle_succeeded` only after a fully successful cycle, so a
        // failed scan/cycle leaves the in-memory cursor at the last confirmed
        // state and the supervisor's retry re-scans from it (issue #332).
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
        self.buffer.lock().await.extend(staged);
        Ok(ScanResult {
            fetched: count,
            cursor_changed: changed,
            cursor,
        })
    }

    /// Process one debounced event batch: collect changed image paths, stage
    /// new/changed ones, and compute the next cursor (returned, not adopted —
    /// see [`initial_scan`](Self::initial_scan) for the failure-safe contract,
    /// issue #332).
    pub(super) async fn process_events(
        &self,
        events: &DebounceEventResult,
    ) -> Result<ScanResult, ConnectorError> {
        // Snapshot the last confirmed cursor; the computed next cursor is
        // returned for the supervisor to persist and hand back via
        // `on_cycle_succeeded` (issue #332). Cloning under the brief lock
        // also releases the guard before the per-file EXIF parses below.
        let mut cursor = self.cursor.lock().await.clone();
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
                    cursor,
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
            cursor,
        })
    }
}
