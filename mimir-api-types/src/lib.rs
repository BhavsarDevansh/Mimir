#![deny(unsafe_code)]
//! Shared serde wire types for the Mimir workspace.
//!
//! This crate decouples the daemon (`mimir-server`) from its clients
//! (`mimir-client`, the `mimir` CLI). All types are grouped by domain:
//!
//! - [`chat`] — sessions, chat requests/responses, tool-call metadata, status,
//!   KG optimization, and SSE stream items.
//! - [`kb`] — knowledge-graph query, browse, category, profile, and audit
//!   wire types.
//! - [`kb_maintenance`] — fact forget/restore/trash and pending-fact
//!   confirmation wire types.
//! - [`connectors`] — connector registration, sync, token-ingest, and action
//!   wire types.

pub mod chat;
pub mod connectors;
pub mod kb;
pub mod kb_maintenance;

pub use chat::{
    ChatMessage, ChatRequest, ChatResponse, OptimizationRunNowResponse, OptimizationRunSummary,
    OptimizationStatusResponse, SessionMessagesResponse, SessionSummary, StatusResponse,
    StreamItem, ToolCallInfo, ToolCallStartInfo, Usage,
};
pub use connectors::{
    ActionResultResponse, AddConnectorRequest, ConnectorActionRequest, ConnectorCatalogEntry,
    ConnectorCatalogResponse, ConnectorListResponse, ConnectorResponse, ForgetConnectorResponse,
    IngestTokenRequest, SyncConnectorRequest, SyncConnectorResponse,
};
pub use kb::{
    AuditQueryRequest, AuditQueryResponse, AuditRow, BrowseEdge, BrowseRequest, BrowseResponse,
    CategoryDetailResponse, CategoryResponse, DependencyRow, FactDetailResponse, FactEditRequest,
    FactEditResponse, FactQueryParams, FactQueryResponse, FactRow, ProfileGroup, ProfileRequest,
    ProfileResponse, SourceRow,
};
pub use kb_maintenance::{
    ConfirmFactResponse, ForgetRequest, ForgetResponse, PendingFactRow, PendingListResponse,
    RejectFactRequest, RestoreRequest, RestoreResponse, TrashListResponse, TrashRow,
};
