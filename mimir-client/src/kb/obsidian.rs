//! Obsidian export/import client commands (issue #62).

use mimir_api_types::{ExportResponse, ImportRequest, ImportResponse};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// Fetch the rendered Obsidian export bundle (`GET /kb/export`).
    pub async fn kb_export(&self) -> Result<ExportResponse, ClientError> {
        self.get_json(&self.url("kb/export"), &()).await
    }

    /// Import an Obsidian vault directory (`POST /kb/import`).
    ///
    /// The daemon scans `path` for Markdown files, plans the import, and
    /// applies it unless `dry_run` is set.
    pub async fn kb_import(
        &self,
        path: &str,
        dry_run: bool,
    ) -> Result<ImportResponse, ClientError> {
        self.post_json(
            &self.url("kb/import"),
            &ImportRequest {
                path: path.to_string(),
                dry_run,
            },
        )
        .await
    }
}
