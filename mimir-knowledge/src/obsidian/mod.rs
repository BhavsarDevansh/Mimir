//! Obsidian-compatible export and import (issue #62).
//!
//! The Knowledge Graph renders as a folder of Markdown documents — one per
//! entity — with YAML frontmatter, wiki-links (`[[Name]]`), and the four
//! section grammar (`Dates`, `Relationships`, `Preferences`, `Facts`). The
//! same grammar parses hand-edited vaults back into the graph through the
//! shared [`normalize_and_insert`](crate::normalize::normalize_and_insert)
//! pipeline, so imports inherit canonicalisation, corroboration /
//! supersession / inference, the sensitivity gate, and the events overlay.
//!
//! Format specification: `docs/obsidian-export-import.md`. The grammar lives
//! in `grammar` (shared by render and parse so the two directions cannot
//! drift); `render` snapshots the graph; `import` plans and applies an
//! import with dry-run support.

mod grammar;
mod import;
mod render;

pub use import::{ObsidianFile, ObsidianImportCounts, ObsidianImportOutcome, scan_markdown_files};
pub use render::ObsidianExport;

pub(crate) use import::import_all;
pub(crate) use render::render_all;
