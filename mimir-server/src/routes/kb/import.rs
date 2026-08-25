//! `POST /kb/import` handler (issue #62).

use std::sync::Arc;

use axum::{Json, extract::State, response::Response};

use mimir_api_types::{ImportRequest, ImportResponse};
use mimir_knowledge::obsidian::{ObsidianFile, scan_markdown_files};

use crate::error;
use crate::state::AppState;

/// Serve the Obsidian vault import backing `mimir kb import`.
///
/// The daemon reads the vault directory (the CLI has no direct database
/// access), parses and plans every Markdown document, and applies (or
/// dry-runs) the import through the shared pipeline.
pub async fn kb_import_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ImportResponse>, Response> {
    let dir = std::path::PathBuf::from(&req.path);
    if !dir.is_dir() {
        return Err(error::not_found(format!(
            "import path is not a directory: {}",
            req.path
        )));
    }

    let mut files = Vec::new();
    let mut errors = Vec::new();
    for path in scan_markdown_files(&dir).map_err(error::knowledge_error)? {
        let relative_path = path
            .strip_prefix(&dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        match std::fs::read_to_string(&path) {
            Ok(content) => files.push(ObsidianFile {
                relative_path,
                content,
            }),
            Err(e) => {
                // One unreadable file never aborts the rest of the vault.
                errors.push(format!("{relative_path}: {e}"));
            }
        }
    }

    let outcome = state
        .knowledge_graph
        .import_obsidian(&files, req.dry_run)
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(ImportResponse {
        dry_run: outcome.dry_run,
        entities_new: outcome.counts.entities_new,
        entities_updated: outcome.counts.entities_updated,
        facts_new: outcome.counts.facts_new,
        facts_existing: outcome.counts.facts_existing,
        preferences_new: outcome.counts.preferences_new,
        preferences_updated: outcome.counts.preferences_updated,
        dates_new: outcome.counts.dates_new,
        errors: {
            let mut all = errors;
            all.extend(outcome.errors);
            all
        },
    }))
}
