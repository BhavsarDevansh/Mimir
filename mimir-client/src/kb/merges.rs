//! Entity merge-queue review commands: list, apply, keep (issue #282).

use mimir_api_types::{MergeApplyResponse, MergeQueueListResponse};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// List pending entity-merge suggestions awaiting review.
    pub async fn kb_merges(&self) -> Result<MergeQueueListResponse, ClientError> {
        self.get_json(&self.url("kb/merges"), &()).await
    }

    /// Apply a pending entity merge suggestion.
    pub async fn kb_merge_apply(&self, merge_id: i64) -> Result<MergeApplyResponse, ClientError> {
        Self::send_json(
            self.client
                .post(self.url(&format!("kb/merges/{merge_id}/apply"))),
        )
        .await
    }

    /// Mark a pending entity merge suggestion as kept separate. Returns
    /// `Ok(())` on a 204 No Content.
    pub async fn kb_merge_keep(&self, merge_id: i64) -> Result<(), ClientError> {
        Self::check_status(
            self.client
                .post(self.url(&format!("kb/merges/{merge_id}/keep")))
                .send()
                .await?,
        )
        .await
    }
}
