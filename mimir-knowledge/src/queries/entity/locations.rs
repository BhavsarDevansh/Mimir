//! Entity-location persistence: upserts, temporal closes, geocoding anchors.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity_location::EntityLocation;

#[allow(clippy::too_many_arguments)]
pub async fn insert_location_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    entity_id: i32,
    location_type_id: i16,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    source_fact_id: Option<i32>,
) -> Result<EntityLocation, KnowledgeError> {
    let record = sqlx::query_as::<_, EntityLocation>(
        "INSERT INTO entity_locations          (entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, source_fact_id)          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)          RETURNING id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, source_fact_id, created_at",
    )
    .bind(entity_id)
    .bind(location_type_id)
    .bind(address)
    .bind(latitude)
    .bind(longitude)
    .bind(timezone)
    .bind(valid_from)
    .bind(valid_until)
    .bind(source_fact_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(record)
}

/// Close any still-open location of the same entity + type that began before
/// `new_valid_from`, setting its `valid_until` to `new_valid_from`.
///
/// Models a move: "home 2020-2023, home 2023-present". Already-closed rows
/// (`valid_until IS NOT NULL`) and rows whose `valid_from` is at or after the
/// new bound are left untouched. A `None` new bound is a no-op (a timeless
/// new location cannot supersede a dated one).
pub async fn close_prior_open_locations_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    entity_id: i32,
    location_type_id: i16,
    new_valid_from: Option<DateTime<Utc>>,
) -> Result<u64, KnowledgeError> {
    let Some(new_valid_from) = new_valid_from else {
        return Ok(0);
    };
    let result = sqlx::query(
        "UPDATE entity_locations          SET valid_until = ?          WHERE entity_id = ? AND location_type_id = ?            AND valid_until IS NULL            AND (valid_from IS NULL OR valid_from < ?)",
    )
    .bind(new_valid_from)
    .bind(entity_id)
    .bind(location_type_id)
    .bind(new_valid_from)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

/// Insert a location for an entity (direct-seed path; no supersession).
///
/// Convenience wrapper around [`insert_location_in_tx`] that opens its own
/// transaction. Use [`KnowledgeGraph::upsert_location`](crate::KnowledgeGraph::upsert_location) when the new location
/// should close a prior open-ended location of the same type (moves).
#[allow(clippy::too_many_arguments)]
pub async fn insert_location(
    pool: &SqlitePool,
    entity_id: i32,
    location_type_id: i16,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    source_fact_id: Option<i32>,
) -> Result<EntityLocation, KnowledgeError> {
    let mut tx = pool.begin().await?;
    let record = insert_location_in_tx(
        &mut tx,
        entity_id,
        location_type_id,
        address,
        latitude,
        longitude,
        timezone,
        valid_from,
        valid_until,
        source_fact_id,
    )
    .await?;
    tx.commit().await?;
    Ok(record)
}

/// Upsert a location for an entity with move/supersession semantics
/// (Phase 3 S3 / #193).
///
/// Begins a transaction, closes any still-open location of the same
/// `entity_id` + `location_type_id` that began before `valid_from` (sets its
/// `valid_until = valid_from`) via [`close_prior_open_locations_in_tx`], then
/// inserts the new row via [`insert_location_in_tx`]. Atomic in one
/// transaction. Shared by the [`KnowledgeGraph::upsert_location`](crate::KnowledgeGraph::upsert_location) facade and
/// the background location-overlay worker so both apply identical move
/// semantics. Geocoding (filling the missing half) is the caller's
/// responsibility; this persists exactly what it is given.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_location(
    pool: &SqlitePool,
    entity_id: i32,
    location_type_id: i16,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    source_fact_id: Option<i32>,
) -> Result<EntityLocation, KnowledgeError> {
    let mut tx = pool.begin().await?;
    close_prior_open_locations_in_tx(&mut tx, entity_id, location_type_id, valid_from).await?;
    let record = insert_location_in_tx(
        &mut tx,
        entity_id,
        location_type_id,
        address,
        latitude,
        longitude,
        timezone,
        valid_from,
        valid_until,
        source_fact_id,
    )
    .await?;
    tx.commit().await?;
    Ok(record)
}

/// Idempotently anchor a `Place` entity's geographic coordinates
/// (Phase 3 C2 / #196).
///
/// A place's coordinates are timeless — a place does not "move" — so the
/// move/supersession semantics of [`upsert_location`] are the wrong model:
/// repeated photos at the same place must not pile up closed `Geographic`
/// rows (which would also pollute `find_nearby`'s validity-agnostic spatial
/// scan). This therefore keeps a single `Geographic` row per place, updated in
/// place when it already exists. The single-row invariant is enforced at the
/// schema level by a partial unique index
/// (`idx_entity_locations_geographic_unique`, migration `047`) on
/// `entity_id` scoped to `location_type_id = Geographic`, and this upsert is a
/// single `INSERT ... ON CONFLICT DO UPDATE` against that index, so it is
/// atomic and race-free even if the overlay worker is later parallelised —
/// the serial-worker convention becomes a performance optimisation, not a
/// correctness requirement. `address` is left `NULL` — the place's name
/// lives on the entity, not the location row.
pub async fn ensure_place_coordinates(
    pool: &SqlitePool,
    place_entity_id: i32,
    latitude: f64,
    longitude: f64,
    source_fact_id: Option<i32>,
) -> Result<(), KnowledgeError> {
    // The `location_type_id` literal (`6` = `LocationType::Geographic`,
    // locked by a `const_assert` in `models/enums.rs`) is hardcoded in the
    // SQL rather than bound, because SQLite requires the `ON CONFLICT`
    // partial-index target `WHERE location_type_id = 6` to match the partial
    // unique index's own `WHERE` clause verbatim — a bound parameter is not
    // permitted there. Keeping the SQL a static literal also satisfies sqlx's
    // `SqlSafeStr` injection guard.
    let upsert = "INSERT INTO entity_locations             (entity_id, location_type_id, latitude, longitude, source_fact_id)             VALUES (?, 6, ?, ?, ?)             ON CONFLICT(entity_id) WHERE location_type_id = 6             DO UPDATE SET                 latitude = excluded.latitude,                 longitude = excluded.longitude,                 source_fact_id = excluded.source_fact_id";
    sqlx::query(upsert)
        .bind(place_entity_id)
        .bind(latitude)
        .bind(longitude)
        .bind(source_fact_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all locations for an entity.
pub async fn get_locations(
    pool: &SqlitePool,
    entity_id: i32,
) -> Result<Vec<EntityLocation>, KnowledgeError> {
    let rows: Vec<EntityLocation> = sqlx::query_as::<_, EntityLocation>(
        "SELECT id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, source_fact_id, created_at          FROM entity_locations WHERE entity_id = ? ORDER BY created_at",
    )
        .bind(entity_id)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Update a location's mutable fields.
pub async fn update_location(
    pool: &SqlitePool,
    id: i32,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
) -> Result<EntityLocation, KnowledgeError> {
    let record = sqlx::query_as::<_, EntityLocation>(
        "UPDATE entity_locations          SET address = COALESCE(?, address),              latitude = COALESCE(?, latitude),              longitude = COALESCE(?, longitude),              timezone = COALESCE(?, timezone)          WHERE id = ?          RETURNING id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, source_fact_id, created_at",
    )
    .bind(address)
    .bind(latitude)
    .bind(longitude)
    .bind(timezone)
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(record)
}
