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

/// Stamp `last_sync_at` and `updated_at` **without** touching `sync_cursor`.
///
/// Use this when a connector reports `SyncOutcome::new_cursor = None`
/// (meaning "cursor unchanged") so the persisted progress token is preserved
/// while the sync timestamp is still advanced. Returns
/// [`KnowledgeError::ConnectorNotFound`] when no row matches `id`.
pub async fn touch_last_sync(
    pool: &SqlitePool,
    id: i32,
    now: DateTime<Utc>,
) -> Result<Connector, KnowledgeError> {
    let row = sqlx::query_as::<_, Connector>(
        "UPDATE connectors SET last_sync_at = ?, updated_at = ? \
         WHERE id = ? \
         RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                   status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                   created_at, updated_at",
    )
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
    // Map the Rust Option<Option<&str>> to a discriminant + value pair:
    // discriminant 0 = leave untouched, 1 = set NULL, 2 = set to value.
    let (discriminant, error_msg) = match error {
        None => (0, ""),
        Some(None) => (1, ""),
        Some(Some(msg)) => (2, msg),
    };

    // Single UPDATE with a CASE expression that conditionally updates last_error
    // based on the discriminant. Bind order: status_id, discriminant, error_msg,
    // updated_at, id.
    let row = sqlx::query_as::<_, Connector>(
        "UPDATE connectors SET \
           status_id = ?, \
           last_error = CASE \
             WHEN ? = 0 THEN last_error \
             WHEN ? = 1 THEN NULL \
             ELSE ? \
           END, \
           updated_at = ? \
         WHERE id = ? \
         RETURNING id, connector_type_id, slug, backend, display_name, config_json, \
                   status_id, auth_state_id, sync_cursor, last_sync_at, last_error, \
                   created_at, updated_at",
    )
    .bind(status as i16)
    .bind(discriminant)
    .bind(discriminant)
    .bind(error_msg)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await?;
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

/// Number of `sources` rows attributed to a connector instance.
///
/// This is the derived "items ingested" metric surfaced by the connector
/// status endpoint (issue #202 / Phase 3 A1): the connectors table itself
/// stores no count column, so the live value is computed from `sources` on
/// demand. A missing instance id yields `0` (no FK references a deleted row).
pub async fn count_sources_for_connector(
    pool: &SqlitePool,
    id: i32,
) -> Result<i64, KnowledgeError> {
    let count = sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// Item counts for every connector instance in one query.
///
/// A single `GROUP BY` over `sources` returns `(connector_instance_id, count)`
/// for every instance that has ingested at least one fact. Connectors with no
/// ingested facts are absent from the map (the caller treats a missing key as
/// `0`). Used by the `GET /connectors` list route so item counts are derived in
/// one round-trip rather than N+1 (issue #202 / Phase 3 A1).
pub async fn count_sources_by_connector(
    pool: &SqlitePool,
) -> Result<std::collections::HashMap<i32, i64>, KnowledgeError> {
    let rows: Vec<(Option<i32>, i64)> = sqlx::query_as(
        "SELECT connector_instance_id, COUNT(*)          FROM sources WHERE connector_instance_id IS NOT NULL          GROUP BY connector_instance_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, count)| id.map(|id| (id, count)))
        .collect())
}

/// Delete a connector instance row, detaching its provenance first.
///
/// The `sources.connector_instance_id` FK has no `ON DELETE` clause (it
/// defaults to `NO ACTION`), so a raw `DELETE` would violate the FK whenever
/// the instance has ingested facts. This nulls every referencing `sources`
/// row — preserving the facts with degraded provenance, consistent with the
/// Phase 3 plan's split that defers the full `forget` cascade to A2 / #203 —
/// then deletes the connector row, in one transaction so a partial detach can
/// never leave the row gone-but-referenced (or vice versa). Returns
/// [`KnowledgeError::ConnectorNotFound`] when no row matches `id`.
pub async fn delete_connector(pool: &SqlitePool, id: i32) -> Result<(), KnowledgeError> {
    let mut tx = pool.begin().await?;

    // Detach provenance. Facts survive with `connector_instance_id = NULL`;
    // the denormalised `connector_type_id` is retained so the connector kind
    // remains queryable after the instance registry row is gone.
    sqlx::query("UPDATE sources SET connector_instance_id = NULL WHERE connector_instance_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    let affected = sqlx::query("DELETE FROM connectors WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    tx.commit().await?;

    if affected == 0 {
        Err(KnowledgeError::ConnectorNotFound(id))
    } else {
        Ok(())
    }
}
