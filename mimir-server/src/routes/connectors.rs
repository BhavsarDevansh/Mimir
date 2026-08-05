//! Connector management routes (Phase 3 A1 / issue #202).
//!
//! `GET /connectors` and `GET /connectors/{id}` surface registered connector
//! instances with derived item counts (computed from the `sources` table).
//! `POST /connectors` registers a new instance keyed on `slug`, validating the
//! `(connector_type, backend)` pair against the daemon's
//! [`ConnectorRegistry`] so an unregistered backend is rejected up front.
//! `DELETE /connectors/{id}` stops the runner (if any) and deletes the row,
//! detaching provenance (the full `forget` cascade is A2 / #203).
//!
//! Adding a connector creates it in `Setup` status; activation (move to
//! `Active` so the supervisor spawns a runner) is an action route that lands
//! with A2 / #203. This keeps A1 a CRUD/status surface, matching the issue's
//! add/remove framing.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};

use mimir_api_types::{AddConnectorRequest, ConnectorListResponse, ConnectorResponse};
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::ConnectorType;

use crate::error;
use crate::state::AppState;

/// Map a lowercase wire `connector_type` string to the enum.
///
/// Kept here (not on the enum) because [`mimir_api_types`] is deliberately
/// decoupled from `mimir-knowledge`, so the wire type is a `String`. Returns
/// `None` for an unknown kind so the handler can surface a `400 Bad Request`.
fn parse_connector_type(s: &str) -> Option<ConnectorType> {
    match s.to_ascii_lowercase().as_str() {
        "gmail" => Some(ConnectorType::Gmail),
        "calendar" => Some(ConnectorType::Calendar),
        "photos" => Some(ConnectorType::Photos),
        "linkedin" => Some(ConnectorType::LinkedIn),
        _ => None,
    }
}

/// Lowercase status string for the wire type, derived from the typed enum so
/// the wire representation tracks the enum without a hard-coded table.
fn status_string(row: &mimir_knowledge::models::connector::Connector) -> String {
    row.status()
        .map(|s| format!("{s:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

fn auth_state_string(row: &mimir_knowledge::models::connector::Connector) -> String {
    row.auth_state()
        .map(|s| format!("{s:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

fn connector_type_string(row: &mimir_knowledge::models::connector::Connector) -> String {
    row.connector_type()
        .map(|t| format!("{t:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build a [`ConnectorResponse`] from a row, deriving its item count from the
/// knowledge graph on demand. Shared by the list and single-instance routes.
async fn to_response(
    state: &AppState,
    row: mimir_knowledge::models::connector::Connector,
) -> Result<ConnectorResponse, Response> {
    let item_count = state
        .knowledge_graph
        .count_sources_for_connector(row.id)
        .await
        .map_err(error::knowledge_error)?;
    // Derive the string views before the owned fields are moved out of `row`.
    let connector_type = connector_type_string(&row);
    let status = status_string(&row);
    let auth_state = auth_state_string(&row);
    Ok(ConnectorResponse {
        id: row.id,
        connector_type,
        slug: row.slug,
        backend: row.backend,
        display_name: row.display_name,
        status,
        auth_state,
        sync_cursor: row.sync_cursor,
        last_sync_at: row.last_sync_at.map(|dt| dt.to_rfc3339()),
        last_error: row.last_error,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        item_count,
    })
}

/// Build a [`ConnectorResponse`] from a row and a precomputed item count.
/// Used by the list route, which derives every count in one query.
fn to_response_with_count(
    row: mimir_knowledge::models::connector::Connector,
    item_count: i64,
) -> ConnectorResponse {
    let connector_type = connector_type_string(&row);
    let status = status_string(&row);
    let auth_state = auth_state_string(&row);
    ConnectorResponse {
        id: row.id,
        connector_type,
        slug: row.slug,
        backend: row.backend,
        display_name: row.display_name,
        status,
        auth_state,
        sync_cursor: row.sync_cursor,
        last_sync_at: row.last_sync_at.map(|dt| dt.to_rfc3339()),
        last_error: row.last_error,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        item_count,
    }
}

/// `GET /connectors` — every registered instance, oldest first, with derived
/// item counts.
///
/// Item counts are derived in a single `GROUP BY` query (one round-trip),
/// not one per row, so the list route stays O(1) round-trips regardless of
/// how many connector instances are registered.
pub async fn connectors_list_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConnectorListResponse>, Response> {
    let rows = state
        .knowledge_graph
        .list_connectors()
        .await
        .map_err(error::knowledge_error)?;
    let counts = state
        .knowledge_graph
        .count_sources_by_connector()
        .await
        .map_err(error::knowledge_error)?;
    let connectors = rows
        .into_iter()
        .map(|row| {
            let item_count = counts.get(&row.id).copied().unwrap_or(0);
            to_response_with_count(row, item_count)
        })
        .collect();
    Ok(Json(ConnectorListResponse { connectors }))
}

/// `GET /connectors/{id}` — a single instance with its derived item count.
pub async fn connector_show_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<ConnectorResponse>, Response> {
    let row = state
        .knowledge_graph
        .get_connector(id)
        .await
        .map_err(error::knowledge_error)?
        .ok_or_else(|| error::not_found("connector not found"))?;
    Ok(Json(to_response(&state, row).await?))
}

/// `POST /connectors` — register a new connector instance.
///
/// Rejects an unregistered `(connector_type, backend)` pair (`400`), an
/// unknown connector kind (`400`), an existing `slug` (`409`), or invalid
/// `config_json` (`400`). The instance is created in `Setup` status; the
/// supervisor does not spawn a runner until a future action route moves it to
/// `Active` (A2 / #203).
pub async fn connector_add_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddConnectorRequest>,
) -> Result<(StatusCode, Json<ConnectorResponse>), Response> {
    let connector_type = parse_connector_type(&body.connector_type).ok_or_else(|| {
        error::bad_request(format!("unknown connector_type: {}", body.connector_type))
    })?;

    // Validate the backend is registered for this type so an unsupported
    // backend is rejected before it is persisted.
    if !state
        .connector_registry
        .is_registered(connector_type, &body.backend)
    {
        return Err(error::bad_request(format!(
            "no connector backend registered for {}/{}",
            body.connector_type, body.backend
        )));
    }

    // Reject an existing slug up front: A1 is add-only. Reconfiguring an
    // existing instance (respawn-on-reconfig) is A2 / #203.
    if state
        .knowledge_graph
        .get_connector_by_slug(&body.slug)
        .await
        .map_err(error::knowledge_error)?
        .is_some()
    {
        return Err(error::conflict(format!(
            "connector slug '{}' already exists",
            body.slug
        )));
    }

    let config_json = serde_json::to_string(&body.config_json)
        .map_err(|e| error::bad_request(format!("invalid config_json: {e}")))?;

    let row = state
        .knowledge_graph
        .upsert_connector(UpsertConnectorInput {
            connector_type,
            slug: body.slug,
            backend: body.backend,
            display_name: body.display_name,
            config_json,
            // New instance: starts in Setup/Unauthenticated; activation is A2.
            status: None,
            auth_state: None,
        })
        .await
        .map_err(error::knowledge_error)?;

    let resp = to_response(&state, row).await?;
    Ok((StatusCode::CREATED, Json(resp)))
}

/// `DELETE /connectors/{id}` — stop the runner (if any) and delete the row.
///
/// Provenance is detached (the `sources.connector_instance_id` FK is nulled)
/// so the ingested facts survive with degraded provenance; the full `forget`
/// cascade is deferred to A2 / #203. Returns `204` on success or `404` when
/// no row matches `id`.
pub async fn connector_remove_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<StatusCode, Response> {
    // Stop the runner first so a mid-cycle sync cannot write back to a row
    // that is about to disappear. `stop` is a no-op (returns false) when no
    // runner exists for `id`, so an unspawned instance is still deletable.
    state.connector_supervisor.stop(id).await;

    state
        .knowledge_graph
        .delete_connector(id)
        .await
        .map_err(error::knowledge_error)?;

    Ok(StatusCode::NO_CONTENT)
}
