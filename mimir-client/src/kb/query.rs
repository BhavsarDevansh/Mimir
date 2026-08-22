//! Knowledge-graph read/edit commands: query, show, edit, browse, profile, audit.

use mimir_api_types::{
    AuditQueryRequest, AuditQueryResponse, BrowseRequest, BrowseResponse, FactDetailResponse,
    FactEditRequest, FactEditResponse, FactQueryParams, FactQueryResponse, HeatmapResponse,
    ProfileRequest, ProfileResponse,
};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// Query facts for an entity.
    pub async fn kb_query(&self, req: FactQueryParams) -> Result<FactQueryResponse, ClientError> {
        let mut params = vec![("entity", req.entity)];
        if let Some(p) = req.predicate {
            params.push(("predicate", p));
        }
        if let Some(c) = req.min_confidence {
            params.push(("min_confidence", c.to_string()));
        }
        if let Some(o) = req.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(l) = req.limit {
            params.push(("limit", l.to_string()));
        }
        self.get_json(&self.url("kb/query"), &params).await
    }

    /// Fetch the knowledge-graph heatmap aggregates (issue #69).
    pub async fn kb_heatmap(&self) -> Result<HeatmapResponse, ClientError> {
        self.get_json(&self.url("kb/heatmap"), &()).await
    }

    /// Show a single fact by ID.
    pub async fn kb_show(&self, fact_id: i32) -> Result<FactDetailResponse, ClientError> {
        self.get_json(&self.url(&format!("kb/facts/{fact_id}")), &())
            .await
    }

    /// Edit a single fact.
    pub async fn kb_edit(
        &self,
        fact_id: i32,
        req: FactEditRequest,
    ) -> Result<FactEditResponse, ClientError> {
        Self::send_json(
            self.client
                .patch(self.url(&format!("kb/facts/{fact_id}")))
                .json(&req),
        )
        .await
    }

    /// Browse the knowledge graph from an entity.
    pub async fn kb_browse(&self, req: BrowseRequest) -> Result<BrowseResponse, ClientError> {
        let mut params: Vec<(&str, String)> =
            vec![("entity", req.entity), ("depth", req.depth.to_string())];
        if let Some(o) = req.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(l) = req.limit {
            params.push(("limit", l.to_string()));
        }
        self.get_json(&self.url("kb/browse"), &params).await
    }

    /// Get a profile for an entity.
    pub async fn kb_profile(&self, req: ProfileRequest) -> Result<ProfileResponse, ClientError> {
        let mut params: Vec<(&str, String)> = vec![];
        if let Some(e) = req.entity {
            params.push(("entity", e));
        }
        self.get_json(&self.url("kb/profile"), &params).await
    }

    /// Query the audit log.
    pub async fn kb_audit(
        &self,
        req: AuditQueryRequest,
    ) -> Result<AuditQueryResponse, ClientError> {
        let mut params: Vec<(&str, String)> = vec![];
        if let Some(e) = req.entity {
            params.push(("entity", e));
        }
        if let Some(p) = req.predicate {
            params.push(("predicate", p));
        }
        if let Some(f) = req.from {
            params.push(("from", f));
        }
        if let Some(t) = req.to {
            params.push(("to", t));
        }
        if let Some(c) = req.change_type {
            params.push(("change_type", c));
        }
        if let Some(o) = req.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(l) = req.limit {
            params.push(("limit", l.to_string()));
        }
        self.get_json(&self.url("kb/audit"), &params).await
    }
}
