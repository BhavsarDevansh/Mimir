//! `mimir kb export` / `mimir kb import` — Obsidian export/import (issue #62).
//!
//! Export renders the knowledge graph as Markdown documents through the
//! daemon (`GET /kb/export`) and writes them to the export directory
//! (`--dir` → `knowledge.export_dir` → `~/AgentKnowledge`). Import sends a
//! vault directory to the daemon (`POST /kb/import`), which parses, plans,
//! and applies (or dry-runs) the import.

use std::path::{Path, PathBuf};

use mimir_api_types::ExportResponse;
use mimir_client::{ClientError, MimirClient};

use crate::cli_util::{exit_with_error, make_client};
use crate::transport::DaemonTransport;

/// Default export directory when neither `--dir` nor `knowledge.export_dir`
/// is configured.
fn default_export_dir() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join("AgentKnowledge"),
        None => PathBuf::from("AgentKnowledge"),
    }
}

/// Resolve the export directory: `--dir` wins, then `knowledge.export_dir`
/// (with `~` expanded), then the `~/AgentKnowledge` default.
fn resolve_export_dir(dir: Option<PathBuf>) -> PathBuf {
    dir.or_else(|| {
        mimir_core::config::Config::load(None)
            .ok()
            .and_then(|config| config.knowledge.export_dir)
    })
    .map(|path| mimir_core::paths::expand_home(&path.to_string_lossy()))
    .unwrap_or_else(default_export_dir)
}

/// Render an export bundle as concatenated documents with
/// `<!-- mimir: {name} -->` separators (stdout mode).
fn render_bundle(resp: &ExportResponse) -> String {
    let mut out = String::new();
    for file in &resp.files {
        out.push_str(&format!("<!-- mimir: {} -->\n", file.relative_path));
        out.push_str(&file.content);
    }
    out
}

/// Write every rendered document under `target`, creating parent directories
/// so nested `relative_path` values (`sub/Alice.md`) never fail the export.
fn write_export_files(
    target: &Path,
    files: &[mimir_api_types::ExportFile],
) -> Result<usize, String> {
    let mut written = 0usize;
    for file in files {
        let destination = target.join(&file.relative_path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&destination, &file.content)
            .map_err(|e| format!("cannot write {}: {e}", destination.display()))?;
        written += 1;
    }
    Ok(written)
}

/// Fetch and render (or write) an export bundle. Pure client-facing logic so
/// tests can drive it against a mock daemon.
pub(crate) async fn run_kb_export(
    client: &MimirClient,
    dir: Option<PathBuf>,
    stdout: bool,
    json: bool,
) -> Result<(), ClientError> {
    let resp = client.kb_export().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp).unwrap());
        return Ok(());
    }
    if stdout {
        print!("{}", render_bundle(&resp));
        return Ok(());
    }

    let target = resolve_export_dir(dir);
    if let Err(e) = std::fs::create_dir_all(&target) {
        exit_with_error(format!(
            "cannot create export directory {}: {e}",
            target.display()
        ));
    }
    let written = match write_export_files(&target, &resp.files) {
        Ok(written) => written,
        Err(e) => exit_with_error(e),
    };
    println!("Exported {written} files to {}:", target.display());
    println!("  Entities: {}", resp.entity_count);
    println!("  Facts: {}", resp.fact_count);
    println!("  Preferences: {}", resp.preference_count);
    println!("  Dates: {}", resp.event_count);
    Ok(())
}

/// Fetch and report (or apply) an import plan. `dry_run` only plans.
pub(crate) async fn run_kb_import(
    client: &MimirClient,
    path: PathBuf,
    dry_run: bool,
    json: bool,
) -> Result<(), ClientError> {
    let resp = client.kb_import(&path.to_string_lossy(), dry_run).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp).unwrap());
        return Ok(());
    }
    if dry_run {
        println!("Would import from {}:", path.display());
    } else {
        println!("Imported from {}:", path.display());
    }
    println!(
        "  Entities: {} new, {} updated",
        resp.entities_new, resp.entities_updated
    );
    println!(
        "  Facts: {} new, {} existing (skipped)",
        resp.facts_new, resp.facts_existing
    );
    println!(
        "  Preferences: {} new, {} updated",
        resp.preferences_new, resp.preferences_updated
    );
    println!("  Dates: {} new", resp.dates_new);
    if !resp.errors.is_empty() {
        println!("  Errors: {}", resp.errors.len());
        for error in &resp.errors {
            println!("    {error}");
        }
    }
    if dry_run {
        println!("Run without --dry-run to apply.");
    }
    Ok(())
}

/// `mimir kb export [--dir <path>] [--stdout] [--json]`
pub async fn handle_kb_export(
    dir: Option<PathBuf>,
    stdout: bool,
    json: bool,
    transport: &DaemonTransport,
) {
    let client = make_client(transport);
    if let Err(e) = run_kb_export(&client, dir, stdout, json).await {
        exit_with_error(e);
    }
}

/// `mimir kb import <path> [--dry-run] [--json]`
pub async fn handle_kb_import(
    path: PathBuf,
    dry_run: bool,
    json: bool,
    transport: &DaemonTransport,
) {
    let client = make_client(transport);
    // The daemon is a separate process, so a relative path must be resolved
    // against the CLI's working directory before it is sent.
    let path = match std::fs::canonicalize(&path) {
        Ok(path) => path,
        Err(e) => exit_with_error(format!("cannot resolve {}: {e}", path.display())),
    };
    if let Err(e) = run_kb_import(&client, path, dry_run, json).await {
        exit_with_error(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_export_dir_prefers_explicit_dir() {
        let dir = resolve_export_dir(Some(PathBuf::from("/tmp/vault")));
        assert_eq!(dir, PathBuf::from("/tmp/vault"));
    }

    #[test]
    fn default_export_dir_is_home_agent_knowledge() {
        let dir = default_export_dir();
        if let Some(home) = dirs::home_dir() {
            assert_eq!(dir, home.join("AgentKnowledge"));
        }
    }

    #[test]
    fn render_bundle_separates_files() {
        let resp = ExportResponse {
            files: vec![mimir_api_types::ExportFile {
                relative_path: "Alice.md".to_string(),
                content: "# Alice\n".to_string(),
            }],
            entity_count: 1,
            fact_count: 0,
            preference_count: 0,
            event_count: 0,
        };
        assert_eq!(render_bundle(&resp), "<!-- mimir: Alice.md -->\n# Alice\n");
    }

    #[test]
    fn write_export_files_creates_parent_directories_for_nested_paths() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![mimir_api_types::ExportFile {
            relative_path: "sub/Alice.md".to_string(),
            content: "# Alice\n".to_string(),
        }];
        let written = write_export_files(dir.path(), &files).unwrap();
        assert_eq!(written, 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sub/Alice.md")).unwrap(),
            "# Alice\n"
        );
    }
}
