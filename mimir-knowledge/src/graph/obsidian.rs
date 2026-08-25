use crate::graph::KnowledgeGraph;
use crate::obsidian;
use crate::*;

impl KnowledgeGraph {
    /// Render the whole graph as Obsidian Markdown documents (issue #62).
    ///
    /// One document per entity: YAML frontmatter with `entity_id`, `type`,
    /// `aliases`, and timestamps, then the `Dates` / `Relationships` /
    /// `Preferences` / `Facts` sections. See `docs/obsidian-export-import.md`
    /// for the canonical format.
    pub async fn export_obsidian(&self) -> Result<obsidian::ObsidianExport, KnowledgeError> {
        obsidian::render_all(self).await
    }

    /// Plan and (unless `dry_run`) apply an Obsidian import (issue #62).
    ///
    /// Files are parsed with the shared grammar, entities resolve through the
    /// canonical chain, and facts insert through
    /// [`normalize_and_insert`](crate::normalize::normalize_and_insert) with
    /// `source_type=Import`. Dry-run returns the exact planned counts without
    /// writing anything.
    pub async fn import_obsidian(
        &self,
        files: &[obsidian::ObsidianFile],
        dry_run: bool,
    ) -> Result<obsidian::ObsidianImportOutcome, KnowledgeError> {
        obsidian::import_all(self, files, dry_run).await
    }
}
