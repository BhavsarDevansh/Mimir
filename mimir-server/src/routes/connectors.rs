//! Connector management routes (Phase 3 A1 / issue #202).
//!
//! `GET /connectors` and `GET /connectors/{id}` surface registered connector
//! instances with derived item counts (computed from the `sources` table).
//! `GET /connectors/catalog` lists every `(connector_type, backend)` pair the
//! daemon can construct (issue #271). `POST /connectors` registers a new
//! instance keyed on `slug`, validating the `(connector_type, backend)` pair
//! against the daemon's [`mimir_connectors::ConnectorRegistry`] so an
//! unregistered backend is rejected up front. `DELETE /connectors/{id}` stops
//! the runner (if any) and
//! deletes the row, detaching provenance so ingested facts survive with
//! degraded provenance.
//!
//! Action routes (A2 / #203): `POST /connectors/{id}/sync` (manual sync),
//! `POST /connectors/{id}/pause` / `resume` (lifecycle control), `POST
//! /connectors/{id}/tokens` (credential ingest + auth-state flip), `POST
//! /connectors/{id}/actions` (write-back dispatch), and `POST
//! /connectors/{id}/forget` (cascade-forget the connector's facts, secret, and
//! row). Adding a connector creates it in `Setup`; activation (spawn a runner)
//! is the `resume` action.

use std::str::FromStr;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};

use mimir_api_types::{
    AddConnectorRequest, ConnectorCatalogEntry, ConnectorCatalogResponse, ConnectorListResponse,
    ConnectorResponse,
};
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::ConnectorType;

use crate::error;
use crate::state::AppState;

/// Map a wire `connector_type` string to the enum.
///
/// The wire type is a `String` because [`mimir_api_types`] is deliberately
/// decoupled from `mimir-knowledge`; the string table itself lives on
/// [`ConnectorType`]'s `FromStr` impl so the input and output directions
/// share one source of truth (issue #264). Input is normalised to lowercase
/// before parsing, matching the pre-existing lenient behaviour. Returns
/// `None` for an unknown kind so the handler can surface a `400 Bad Request`.
fn parse_connector_type(s: &str) -> Option<ConnectorType> {
    ConnectorType::from_str(&s.to_ascii_lowercase()).ok()
}

/// Lowercase status string for the wire type, via [`ConnectorStatus::as_str`]
/// so the wire representation tracks the enum without a hard-coded table.
fn status_string(row: &mimir_knowledge::models::connector::Connector) -> String {
    row.status()
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn auth_state_string(row: &mimir_knowledge::models::connector::Connector) -> String {
    row.auth_state()
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn connector_type_string(row: &mimir_knowledge::models::connector::Connector) -> String {
    row.connector_type()
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Resolve the mode a row would run in by constructing it from the persisted
/// config (no side effects) — the `push` / `polling` value surfaced by
/// [`ConnectorResponse`] (issue #397). `None` when the row cannot be
/// constructed (unknown type / invalid config).
fn resolved_mode_string(
    state: &AppState,
    row: &mimir_knowledge::models::connector::Connector,
) -> Option<String> {
    state
        .connector_supervisor
        .resolved_mode(row)
        .map(|m| m.wire_name().to_string())
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
    let mode = resolved_mode_string(state, &row);
    Ok(ConnectorResponse {
        id: row.id,
        connector_type,
        slug: row.slug,
        backend: row.backend,
        display_name: row.display_name,
        status,
        auth_state,
        mode,
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
    mode: Option<String>,
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
        mode,
        sync_cursor: row.sync_cursor,
        last_sync_at: row.last_sync_at.map(|dt| dt.to_rfc3339()),
        last_error: row.last_error,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        item_count,
    }
}

/// Fetch a connector row by id and build a [`ConnectorResponse`] (with its
/// derived item count). Shared by the show route and the action routes that
/// return the updated instance after a mutation (`pause` / `resume` /
/// `tokens`). Returns `404` when no row matches `id`.
async fn connector_response(
    state: &AppState,
    id: i32,
) -> Result<Json<ConnectorResponse>, Response> {
    let row = state
        .knowledge_graph
        .get_connector(id)
        .await
        .map_err(error::knowledge_error)?
        .ok_or_else(|| error::not_found("connector not found"))?;
    Ok(Json(to_response(state, row).await?))
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
            let mode = resolved_mode_string(&state, &row);
            to_response_with_count(row, item_count, mode)
        })
        .collect();
    Ok(Json(ConnectorListResponse { connectors }))
}

/// `GET /connectors/catalog` — every registered `(connector_type, backend)`
/// pair the daemon can construct, sorted by type then backend.
///
/// The registry is populated at startup from the daemon's cargo features
/// (`photos` / `calendar` / `gmail`, plus the test mock under
/// `mock-connector`), so the catalog is the authoritative discovery surface
/// for `mimir connector add` (issue #271) — users never have to guess a
/// backend string, and shell completion / interactive wizards can build on
/// it later. The static path takes precedence over `GET /connectors/{id}`,
/// so `catalog` is never mistaken for an instance id.
pub async fn connector_catalog_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConnectorCatalogResponse>, Response> {
    let entries = state
        .connector_registry
        .pairs()
        .into_iter()
        .map(|(connector_type, backend)| ConnectorCatalogEntry {
            connector_type: connector_type.as_str().to_string(),
            backend,
        })
        .collect();
    Ok(Json(ConnectorCatalogResponse { entries }))
}

/// `GET /connectors/{id}` — a single instance with its derived item count.
pub async fn connector_show_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<ConnectorResponse>, Response> {
    connector_response(&state, id).await
}

/// `POST /connectors` — register a new connector instance.
///
/// Rejects an unregistered `(connector_type, backend)` pair (`400`), an
/// unknown connector kind (`400`), an existing `slug` (`409`), or invalid
/// `config_json` (`400`). The instance is created in `Setup` status; the
/// supervisor does not spawn a runner until a future action route moves it to
/// `Active` (A2 / #203).
///
/// Slug uniqueness is enforced atomically by `create_connector`, which relies
/// on the `connectors.slug UNIQUE` index rather than a pre-read plus upsert,
/// so two concurrent `POST /connectors` writes for the same slug cannot both
/// succeed — one wins and the other gets `409 Conflict`.
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

    let config_json = serde_json::to_string(&body.config_json)
        .map_err(|e| error::bad_request(format!("invalid config_json: {e}")))?;

    // Atomic create-only insert: the unique-slug constraint is enforced at the
    // database level, closing the read-then-write window a pre-read
    // `get_connector_by_slug` + `upsert_connector` would leave. A duplicate slug
    // surfaces as `ConnectorSlugConflict`, mapped to `409 Conflict` by the
    // server error layer. Reconfiguring an existing instance is A2 / #203.
    let row = state
        .knowledge_graph
        .create_connector(UpsertConnectorInput {
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

/// `DELETE /connectors/{id}` — stop the runner (if any), delete the stored
/// credentials, and delete the row.
///
/// The per-connector lifecycle lock is held across the whole
/// stop → secret-delete → row-delete sequence, so a concurrent `resume` can
/// never re-spawn a runner for a row that is about to disappear (issue
/// #266). Provenance is detached (the `sources.connector_instance_id` FK is
/// nulled) so the ingested facts survive with degraded provenance; the full
/// `forget` cascade is deferred to A2 / #203. The connector's secret-store
/// entry (keyed by its slug) is deleted **before** the row so that a
/// secret-deletion failure leaves the instance intact (retryable) rather
/// than a deleted row with lingering credentials that a later same-slug
/// connector could load; deleting the secret is idempotent, so an instance
/// that never stored credentials cleans up as a no-op. Returns `204` on
/// success, `404` when no row matches `id`, or `500` if credential deletion
/// fails (the instance is not removed in that case).
pub async fn connector_remove_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<StatusCode, Response> {
    // Fetch the row first: the slug is needed to delete the connector's stored
    // credentials, and a missing id surfaces a clean 404 before any mutation.
    let row = state
        .knowledge_graph
        .get_connector(id)
        .await
        .map_err(error::knowledge_error)?
        .ok_or_else(|| error::not_found("connector not found"))?;

    // Serialise the whole removal against lifecycle mutations for this
    // instance (issue #266): the per-connector lifecycle lock is held across
    // stop → secret-delete → row-delete, so a concurrent `resume`/`start`
    // can never re-spawn a runner for a row that is about to disappear.
    let _guard = state.connector_supervisor.lifecycle_lock(id).await;

    // Stop the runner first so a mid-cycle sync cannot write back to a row
    // that is about to disappear. `stop` is a no-op (returns false) when no
    // runner exists for `id`, so an unspawned instance is still deletable.
    state.connector_supervisor.stop(id).await;

    // Delete the connector's stored credentials *before* the row. The secret
    // is keyed by the connector's slug, so removing it here prevents a later
    // connector created with the same slug from loading the deleted instance's
    // credentials. `SecretStore::delete` is idempotent (a missing entry is
    // `Ok`), so an instance that never stored credentials deletes cleanly. A
    // failure aborts the removal and returns `500` so the request never
    // reports success while the database and secret store are left in an
    // ambiguous state; the instance remains intact and the user can retry.
    if let Some(secret_store) = state.connector_supervisor.secret_store() {
        if let Err(error) = secret_store.delete(&row.slug).await {
            tracing::error!(
                connector_id = id,
                slug = %row.slug,
                %error,
                "failed to delete connector secret; the instance was not removed",
            );
            return Err(error::internal("failed to delete connector credentials"));
        }
    }

    // Detach provenance (null the `sources.connector_instance_id` FK) and
    // delete the row in one transaction so ingested facts survive with
    // degraded provenance. The full `forget` cascade is A2 / #203.
    state
        .knowledge_graph
        .delete_connector(id)
        .await
        .map_err(error::knowledge_error)?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Action routes (Phase 3 A2 / issue #203)
// ---------------------------------------------------------------------------

/// `POST /connectors/{id}/sync` — trigger a manual sync (F9).
pub async fn connector_sync_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
    Json(body): Json<mimir_api_types::SyncConnectorRequest>,
) -> Result<Json<mimir_api_types::SyncConnectorResponse>, Response> {
    let options = mimir_connectors::SyncOptions {
        full: body.full,
        since: body.since.map(std::time::Duration::from_secs),
    };
    let outcome = state
        .connector_supervisor
        .trigger_sync(id, options)
        .await
        .map_err(error::trigger_error)?;
    Ok(Json(match outcome {
        mimir_connectors::TriggerOutcome::Ok {
            fetched,
            new_cursor,
        } => mimir_api_types::SyncConnectorResponse::Ok {
            fetched,
            new_cursor,
        },
        mimir_connectors::TriggerOutcome::AuthExpired => {
            mimir_api_types::SyncConnectorResponse::AuthExpired
        }
        mimir_connectors::TriggerOutcome::Failed(message) => {
            mimir_api_types::SyncConnectorResponse::Failed { message }
        }
    }))
}

/// `POST /connectors/{id}/pause` — stop the runner and transition to `Paused`.
pub async fn connector_pause_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<ConnectorResponse>, Response> {
    state
        .connector_supervisor
        .pause(id)
        .await
        .map_err(error::supervisor_error)?;
    connector_response(&state, id).await
}

/// `POST /connectors/{id}/resume` — (re)spawn the runner and transition to `Active`.
pub async fn connector_resume_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<ConnectorResponse>, Response> {
    state
        .connector_supervisor
        .resume(id)
        .await
        .map_err(error::supervisor_error)?;
    connector_response(&state, id).await
}

/// Convert a wire [`mimir_api_types::IngestTokenRequest`] into the
/// [`mimir_connectors::SecretBundle`] it mirrors, parsing the RFC-3339 OAuth
/// expiry into a `DateTime<Utc>`. Returns an error *message* (not a `Response`)
/// on a malformed expiry so the caller maps it onto a `400` — keeping the
/// `Result` small and avoiding `clippy::result_large_err`.
fn to_secret_bundle(
    req: mimir_api_types::IngestTokenRequest,
) -> Result<mimir_connectors::SecretBundle, String> {
    Ok(match req {
        mimir_api_types::IngestTokenRequest::OAuth {
            access_token,
            refresh_token,
            expires_at,
            client_secret,
        } => {
            let expires_at = match expires_at {
                Some(s) => Some(
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .map_err(|e| format!("invalid expires_at: {e}"))?,
                ),
                None => None,
            };
            mimir_connectors::SecretBundle::OAuth {
                access_token,
                refresh_token,
                expires_at,
                client_secret,
            }
        }
        mimir_api_types::IngestTokenRequest::ApiToken { token } => {
            mimir_connectors::SecretBundle::ApiToken { token }
        }
        mimir_api_types::IngestTokenRequest::AppPassword { password } => {
            mimir_connectors::SecretBundle::AppPassword { password }
        }
    })
}

/// `POST /connectors/{id}/tokens` — ingest credentials and flip `auth_state`.
///
/// `auth_state` becomes `authenticated` as soon as the bundle is written to
/// the store — meaning *credentials are present*, not that they have been
/// validated. The actual handshake runs at the next sync; a credential kind
/// the backend rejects surfaces there as `NotAuthenticated` / `Expired`.
pub async fn connector_tokens_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
    Json(body): Json<mimir_api_types::IngestTokenRequest>,
) -> Result<Json<ConnectorResponse>, Response> {
    let row = state
        .knowledge_graph
        .get_connector(id)
        .await
        .map_err(error::knowledge_error)?
        .ok_or_else(|| error::not_found("connector not found"))?;

    let bundle = to_secret_bundle(body).map_err(error::bad_request)?;
    let secret_store = state
        .connector_supervisor
        .secret_store()
        .ok_or_else(|| error::internal("no secret store configured"))?;
    secret_store
        .store(&row.slug, &bundle)
        .await
        .map_err(error::secret_error)?;

    // Credentials are now present; flip auth_state to Authenticated.
    state
        .knowledge_graph
        .set_auth_state(
            id,
            mimir_knowledge::models::enums::ConnectorAuthState::Authenticated,
        )
        .await
        .map_err(error::knowledge_error)?;

    connector_response(&state, id).await
}

/// `POST /connectors/{id}/actions` — dispatch a write-back action (C4).
pub async fn connector_actions_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
    Json(body): Json<mimir_api_types::ConnectorActionRequest>,
) -> Result<Json<mimir_api_types::ActionResultResponse>, Response> {
    let action = mimir_connectors::ConnectorAction {
        kind: body.kind,
        payload: body.payload,
    };
    let result = state
        .connector_supervisor
        .act(id, action)
        .await
        .map_err(error::act_error)?;
    Ok(Json(mimir_api_types::ActionResultResponse {
        success: result.success,
        native_id: result.native_id,
        message: result.message,
    }))
}

/// `POST /connectors/{id}/forget` — cascade-forget a connector's facts, secret,
/// and row.
///
/// Soft-deletes (trashes) every fact sourced from the connector, deletes its
/// stored credential, then deletes the connector row. Unlike
/// [`connector_remove_handler`] (which detaches provenance so facts survive),
/// this removes the connector's facts entirely (recoverable from trash).
///
/// The cascade is serialised per connector via the supervisor's lifecycle
/// lock, so a concurrent `resume` cannot re-spawn the runner mid-cascade. The
/// instance is first marked `Paused` (with `last_error = "forget in
/// progress"`) so an aborted cascade leaves a state a retry can reason about;
/// the secret is deleted before the irreversible fact trash, so a
/// credential-deletion failure aborts with nothing destroyed. If a later step
/// fails (fact trash or row deletion), the residual state is a `Paused`,
/// credential-less row whose facts may already be trashed — retrying the
/// cascade is idempotent and self-heals.
pub async fn connector_forget_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<mimir_api_types::ForgetConnectorResponse>, Response> {
    // Verify the connector exists (clean 404 before any mutation).
    let row = state
        .knowledge_graph
        .get_connector(id)
        .await
        .map_err(error::knowledge_error)?
        .ok_or_else(|| error::not_found("connector not found"))?;

    // Serialise the whole cascade against lifecycle mutations (resume) for
    // this instance, so a concurrent re-spawn cannot sync against a row that
    // is about to be deleted.
    let _guard = state.connector_supervisor.lifecycle_lock(id).await;

    // Mark the instance Paused before the cascade so a concurrent `resume`
    // observes a non-active row, and so an aborted cascade leaves the
    // connector in a state a retry can reason about.
    state
        .knowledge_graph
        .set_connector_status(
            id,
            mimir_knowledge::models::enums::ConnectorStatus::Paused,
            Some(Some("forget in progress".to_string())),
        )
        .await
        .map_err(error::knowledge_error)?;

    // Stop the runner and run the connector's local forget() cleanup
    // (watcher teardown, in-memory buffers, connector-owned credentials).
    state
        .connector_supervisor
        .forget(id)
        .await
        .map_err(error::supervisor_error)?;

    // Delete the connector's stored credentials (idempotent) before the
    // irreversible fact trash: a failure here aborts the request with nothing
    // destroyed, so the caller can retry cleanly. Connectors whose forget()
    // deletes their own secret (Calendar/Email) make this a no-op.
    if let Some(secret_store) = state.connector_supervisor.secret_store() {
        if let Err(err) = secret_store.delete(&row.slug).await {
            tracing::error!(
                connector_id = id,
                slug = %row.slug,
                error = %err,
                "failed to delete connector secret; no facts were forgotten"
            );
            return Err(error::internal("failed to delete connector credentials"));
        }
    }

    // Trash every fact sourced from this connector instance.
    let result = state
        .knowledge_graph
        .forget_connector_facts(id, mimir_knowledge::models::audit_log::ChangedBy::User)
        .await
        .map_err(error::knowledge_error)?;

    // Delete the connector row (nulls the FK cascade already handled by the
    // fact trash, then removes the row).
    state
        .knowledge_graph
        .delete_connector(id)
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(mimir_api_types::ForgetConnectorResponse {
        forgotten_count: result.forgotten_count,
    }))
}
