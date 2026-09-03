//! Unrecognized-predicate staging review commands (#468).

use mimir_api_types::{
    MapUnrecognizedFactRequest, MapUnrecognizedFactResponse, RejectUnrecognizedFactRequest,
    UnrecognizedFactListResponse,
};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// List durable unrecognized-predicate facts awaiting review.
    pub async fn kb_staged(&self) -> Result<UnrecognizedFactListResponse, ClientError> {
        self.get_json(&self.url("kb/staged"), &()).await
    }

    /// Map a staged fact to an existing emit-eligible relationship leaf.
    pub async fn kb_staged_map(
        &self,
        id: i64,
        relationship_type_id: i16,
        note: Option<&str>,
    ) -> Result<MapUnrecognizedFactResponse, ClientError> {
        let req = MapUnrecognizedFactRequest {
            relationship_type_id,
            note: note.map(|s| s.to_string()),
        };
        self.post_json(&self.url(&format!("kb/staged/{id}/map")), &req)
            .await
    }

    /// Reject a staged fact. Returns `Ok(())` on a 204 No Content.
    pub async fn kb_staged_reject(&self, id: i64, note: Option<&str>) -> Result<(), ClientError> {
        let req = RejectUnrecognizedFactRequest {
            note: note.map(|s| s.to_string()),
        };
        Self::check_status(
            self.client
                .post(self.url(&format!("kb/staged/{id}/reject")))
                .json(&req)
                .send()
                .await?,
        )
        .await
    }
}
