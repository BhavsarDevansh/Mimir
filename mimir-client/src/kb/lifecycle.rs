//! Fact lifecycle commands: forget, restore, trash, pending confirmation.

use mimir_api_types::{
    ConfirmFactResponse, ForgetRequest, ForgetResponse, PendingListResponse, RejectFactRequest,
    RestoreRequest, RestoreResponse, TrashListResponse,
};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// Forget facts (single or bulk).
    pub async fn kb_forget(&self, req: ForgetRequest) -> Result<ForgetResponse, ClientError> {
        self.post_json(&self.url("kb/facts/forget"), &req).await
    }

    /// Restore facts from trash.
    pub async fn kb_restore(&self, req: RestoreRequest) -> Result<RestoreResponse, ClientError> {
        self.post_json(&self.url("kb/trash/restore"), &req).await
    }

    /// List trash contents.
    pub async fn kb_trash(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<TrashListResponse, ClientError> {
        let params = [("offset", offset.to_string()), ("limit", limit.to_string())];
        self.get_json(&self.url("kb/trash"), &params).await
    }

    /// Empty the trash.
    pub async fn kb_trash_empty(&self) -> Result<(), ClientError> {
        Self::check_status(self.client.delete(self.url("kb/trash")).send().await?).await
    }

    /// List pending sensitive facts awaiting confirmation.
    pub async fn kb_pending(&self) -> Result<PendingListResponse, ClientError> {
        self.get_json(&self.url("kb/pending"), &()).await
    }

    /// Confirm a pending sensitive fact.
    pub async fn kb_confirm(&self, fact_id: i32) -> Result<ConfirmFactResponse, ClientError> {
        Self::send_json(
            self.client
                .post(self.url(&format!("kb/facts/{fact_id}/confirm"))),
        )
        .await
    }

    /// Reject a pending sensitive fact. An optional reason is written to the
    /// audit log. Returns `Ok(())` on a 204 No Content.
    pub async fn kb_reject(&self, fact_id: i32, reason: Option<&str>) -> Result<(), ClientError> {
        let req = RejectFactRequest {
            reason: reason.map(|s| s.to_string()),
        };
        Self::check_status(
            self.client
                .post(self.url(&format!("kb/facts/{fact_id}/reject")))
                .json(&req)
                .send()
                .await?,
        )
        .await
    }
}
