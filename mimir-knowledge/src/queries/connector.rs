//! Connector instance-registry CRUD and lifecycle queries (issue #179 / F2).
//!
//! All SQL is written as `&'static str` literals (sqlx 0.9 requires
//! `SqlSafeStr` for `query_as`); dynamic values flow through bind parameters
//! only.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::connector::{Connector, UpsertConnectorInput};
use crate::models::enums::{ConnectorAuthState, ConnectorStatus};

/// List every registered connector instance, oldest first.
pub async fn list_connectors(pool: &SqlitePool) -> Result<Vec<Connector>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Connector>(
        "SELECT id, connector_type_id, slug, backend, display_name, config_json, \
         status_id, auth_state_id, sync_cursor, last_sync_at, last_error, created_at, updated_at \
         FROM connectors ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fetch a connector by its unique human label.
pub async fn get_connector_by_slug(
    pool: &SqlitePool,
    slug: &str,
) -> Result<Option<Connector>, KnowledgeError> {
    let row = sqlx::query_as::<_, Connector>(
        "SELECT id, connector_type_id, slug, backend, display_name, config_json, \
         status_id, auth_state_id, sync_cursor, last_sync_at, last_error, created_at, updated_at \
         FROM connectors WHERE slug = ?",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Fetch a connector by its integer primary key.
pub async fn get_connector(
    pool: &SqlitePool,
    id: i32,
) -> Result<Option<Connector>, KnowledgeError> {
    let row = sqlx::query_as::<_, Connector>(
        "SELECT id, connector_type_id, slug, backend, display_name, config_json, \
         status_id, auth_state_id, sync_cursor, last_sync_at, last_error, created_at, updated_at \
         FROM connectors WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Insert a new connector or update the mutable config surface of an existing
/// one (keyed on `slug`).
///
/// `slug` and `connector_type` are immutable identity: on conflict only the
/// mutable surface (`backend`, `display_name`, `config_json`, `status`,
/// `auth_state`) is overwritten and `updated_at` is bumped, while `id`,
/// `created_at`, and the sync-progress fields (`sync_cursor`, `last_sync_at`,
/// `last_error`) are preserved. Reusing an existing `slug` with a *different*
/// `ConnectorType` returns [`KnowledgeError::ConnectorTypeMismatch`] rather
/// than silently rewriting the instance's kind (which would leave the previous
/// backend's type-specific sync state attached to a different connector type).
///
/// `connector_type` is the typed `ConnectorType` enum whose variants map to the
/// seeded `connector_types` rows, so the FK is guaranteed valid; the
/// `connector_types(id)` foreign key is the DB-level guard.
pub async fn upsert_connector(
    pool: &SqlitePool,
    input: &UpsertConnectorInput,
    now: DateTime<Utc>,
) -> Result<Connector, KnowledgeError> {
    let connector_type_id = input.connector_type as i16;
    let status_id = input.status.unwrap_or(ConnectorStatus::Setup) as i16;
    let auth_state_id = input
        .auth_state
        .unwrap_or(ConnectorAuthState::Unauthenticated) as i16;

    // The `WHERE connectors.connector_type_id = excluded.connector_type_id`
    // guard makes the type immutable on conflict: a type-mismatch conflict
    // updates zero rows, so `RETURNING` yields nothing and we surface a clean
    // `ConnectorTypeMismatch` error (a pure insert or a same-type conflict
    // always returns exactly one row).
    let row = sqlx::query_as::<_, Connector>(
        "INSERT INTO connectors \
         (connector_type_id, slug, backend, display_name, config_json, status_id, auth_state_id, \
          created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(slug) DO UPDATE SET \
            backend = excluded.backend, \
            display_name = excluded.display_name, \
            config_json = excluded.config_json, \
            status_id = excluded.status_id, \
            auth_state_id = excluded.auth_state_id, \
            updated_at = excluded.updated_at \
         WHERE connectors.connector_type_id = excluded.connector_type_id \
         RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                   status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                   created_at, updated_at",
    )
    .bind(connector_type_id)
    .bind(&input.slug)
    .bind(&input.backend)
    .bind(&input.display_name)
    .bind(&input.config_json)
    .bind(status_id)
    .bind(auth_state_id)
    .bind(now)
    .bind(now)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| KnowledgeError::ConnectorTypeMismatch(input.slug.clone()))?;
    Ok(row)
}

/// Advance the opaque sync cursor, stamping `last_sync_at` and `updated_at`.
///
/// `cursor = None` clears the cursor (e.g. a full re-sync). Returns
/// [`KnowledgeError::ConnectorNotFound`] when no row matches `id`.
pub async fn update_sync_cursor(
    pool: &SqlitePool,
    id: i32,
    cursor: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Connector, KnowledgeError> {
    let row = sqlx::query_as::<_, Connector>(
        "UPDATE connectors SET sync_cursor = ?, last_sync_at = ?, updated_at = ? \
         WHERE id = ? \
         RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                   status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                   created_at, updated_at",
    )
    .bind(cursor)
    .bind(now)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeError::ConnectorNotFound(id))?;
    Ok(row)
}

/// Transition a connector to a new lifecycle status, optionally touching
/// `last_error`.
///
/// `error` follows the nullable-update pattern: `None` leaves `last_error`
/// untouched, `Some(None)` clears it to NULL, and `Some(Some(msg))` records
/// `msg`. Returns [`KnowledgeError::ConnectorNotFound`] when no row matches
/// `id`.
pub async fn set_connector_status(
    pool: &SqlitePool,
    id: i32,
    status: ConnectorStatus,
    error: Option<Option<&str>>,
    now: DateTime<Utc>,
) -> Result<Connector, KnowledgeError> {
    // Bind order must follow placeholder order, so branch on `error` to build
    // the matching literal SQL + bind sequence (no string interpolation).
    let row = match error {
        // Leave last_error untouched.
        None => {
            sqlx::query_as::<_, Connector>(
                "UPDATE connectors SET status_id = ?, updated_at = ? WHERE id = ? \
             RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                       status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                       created_at, updated_at",
            )
            .bind(status as i16)
            .bind(now)
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        // Clear last_error to NULL.
        Some(None) => {
            sqlx::query_as::<_, Connector>(
                "UPDATE connectors SET status_id = ?, last_error = NULL, updated_at = ? \
             WHERE id = ? \
             RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                       status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                       created_at, updated_at",
            )
            .bind(status as i16)
            .bind(now)
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        // Record an error message.
        Some(Some(msg)) => {
            sqlx::query_as::<_, Connector>(
                "UPDATE connectors SET status_id = ?, last_error = ?, updated_at = ? \
             WHERE id = ? \
             RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                       status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                       created_at, updated_at",
            )
            .bind(status as i16)
            .bind(msg)
            .bind(now)
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
    };
    row.ok_or(KnowledgeError::ConnectorNotFound(id))
}

/// Set the auth state of a connector. Returns
/// [`KnowledgeError::ConnectorNotFound`] when no row matches `id`.
pub async fn set_auth_state(
    pool: &SqlitePool,
    id: i32,
    auth_state: ConnectorAuthState,
    now: DateTime<Utc>,
) -> Result<Connector, KnowledgeError> {
    let row = sqlx::query_as::<_, Connector>(
        "UPDATE connectors SET auth_state_id = ?, updated_at = ? WHERE id = ? \
         RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                   status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                   created_at, updated_at",
    )
    .bind(auth_state as i16)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(KnowledgeError::ConnectorNotFound(id))?;
    Ok(row)
}
