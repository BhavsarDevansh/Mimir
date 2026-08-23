//! Connector instance-registry model and upsert input (issue #179 / Phase 3 F2).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::models::enums::{ConnectorAuthState, ConnectorStatus, ConnectorType};

/// A single configured connector instance registered in the knowledge graph.
///
/// One row per configured service account (e.g. one Gmail account, one CalDAV
/// calendar). Backends persist their sync cursor, auth state, and health here so
/// they survive daemon restarts. Lookup columns are stored as raw `i16` ids and
/// exposed via typed accessors, mirroring the `Event` overlay model.
#[derive(Debug, Clone, PartialEq, FromRow, Serialize, Deserialize)]
pub struct Connector {
    pub id: i32,
    pub connector_type_id: i16,
    pub slug: String,
    pub backend: String,
    pub display_name: String,
    pub config_json: String,
    pub status_id: i16,
    pub auth_state_id: i16,
    pub sync_cursor: Option<String>,
    /// Opaque, connector-owned durable state (e.g. the Email connector's
    /// LLM-extraction retry ledger, issue #262), persisted by the supervisor
    /// after each successful extraction cycle and re-injected at
    /// construction. `None` when the connector keeps no durable state.
    pub durable_state: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Connector {
    /// Typed lifecycle status, or `None` if the stored id is unknown.
    pub fn status(&self) -> Option<ConnectorStatus> {
        ConnectorStatus::try_from(self.status_id).ok()
    }

    /// Typed auth state, or `None` if the stored id is unknown.
    pub fn auth_state(&self) -> Option<ConnectorAuthState> {
        ConnectorAuthState::try_from(self.auth_state_id).ok()
    }

    /// Typed connector kind, or `None` if the stored id is unknown.
    pub fn connector_type(&self) -> Option<ConnectorType> {
        match self.connector_type_id {
            x if x == ConnectorType::Email as i16 => Some(ConnectorType::Email),
            x if x == ConnectorType::Calendar as i16 => Some(ConnectorType::Calendar),
            x if x == ConnectorType::Photos as i16 => Some(ConnectorType::Photos),
            x if x == ConnectorType::LinkedIn as i16 => Some(ConnectorType::LinkedIn),
            _ => None,
        }
    }
}

/// Input for [`crate::KnowledgeGraph::upsert_connector`].
///
/// Upsert is keyed on `slug`. `slug` and `connector_type` are immutable
/// identity: on conflict the mutable config surface (`backend`,
/// `display_name`, `config_json`, `status`, `auth_state`) is overwritten and
/// `updated_at` is bumped, while `id`, `created_at`, and the sync-progress
/// fields (`sync_cursor`, `durable_state`, `last_sync_at`, `last_error`) are
/// preserved because they are owned by dedicated mutators. Reusing an
/// existing `slug` with a different `connector_type` returns
/// [`crate::KnowledgeError::ConnectorTypeMismatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpsertConnectorInput {
    pub connector_type: ConnectorType,
    pub slug: String,
    pub backend: String,
    pub display_name: String,
    /// Connector-specific configuration as a JSON object string (e.g. `"{}"`).
    pub config_json: String,
    /// Initial status. `None` defaults to `Setup` on insert; on conflict this
    /// overwrites the existing status.
    pub status: Option<ConnectorStatus>,
    /// Initial auth state. `None` defaults to `Unauthenticated` on insert; on
    /// conflict this overwrites the existing auth state.
    pub auth_state: Option<ConnectorAuthState>,
}
