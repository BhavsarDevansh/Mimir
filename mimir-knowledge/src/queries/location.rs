//! Entity-location persistence: upserts, temporal closes, geocoding anchors.
//!
//! Module layout by concern:
//!
//! - this module — `entity_locations` / `pending_location_meta` persistence.
//! - `nearby` — geographic near-by search.

mod nearby;

pub use nearby::find_nearby;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity_location::EntityLocation;
use crate::queries::fact::ranges_overlap;

/// Insert a location row inside an existing transaction.
///
/// The transaction is the caller's to commit or roll back; used by
/// [`upsert_location`] for the atomic move/supersession write and exposed for
/// callers that need to batch a location insert with other writes.
/// [`insert_location`] is the convenience wrapper that opens its own
/// transaction (direct-seed path; no supersession).
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
/// (Phase 3 S3 / #193), deduplicating same-place re-statements (issue #228).
///
/// If the incoming location is the *same place* as an existing row of the same
/// `entity_id` + `location_type_id` whose period overlaps it, the statement is
/// a re-statement rather than a move — the existing row absorbs the incoming
/// bounds (interval union) and any shape fields it is missing, and no new row
/// is inserted and nothing is closed. Otherwise the move/supersession path
/// applies, atomically in one transaction: any still-open location of the same
/// `entity_id` + `location_type_id` that began before `valid_from` is closed
/// (its `valid_until` set to `valid_from`) via
/// [`close_prior_open_locations_in_tx`], then the new row is inserted via
/// [`insert_location_in_tx`]. Shared by the
/// [`KnowledgeGraph::upsert_location`](crate::KnowledgeGraph::upsert_location) facade and the background
/// location-overlay worker so both apply identical semantics. Geocoding
/// (filling the missing half) is the caller's responsibility; this persists
/// exactly what it is given.
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
    // Re-statement dedup: fold the incoming statement into the earliest
    // overlapping same-place row when one exists. The lookup runs *before* the
    // transaction (on a separate read) so the transaction's first statement is
    // always a write: a deferred transaction that reads first and writes
    // second fails with an un-retriable SQLITE_BUSY when another connection
    // commits in between (WAL stale-snapshot upgrade, issue #236), and the
    // supervisor's bookkeeping writes (cursor/status) do exactly that. The
    // merge or move below is still atomic in its own transaction, and the
    // shared knowledge-graph write lock serialises every caller — the facade
    // acquires it in `KnowledgeGraph::upsert_location`, and the overlay
    // worker holds it across `apply_location_overlay` — so the lookup cannot
    // go stale on any path.
    //
    // The WHERE clause pre-filters to rows whose period can overlap the
    // incoming one (a SQL mirror of `ranges_overlap`), keeping the fetch
    // proportional to the overlapping history rather than the entity's full
    // location history; the exact overlap + same-place predicate is still
    // applied in Rust below, so the SQL is a pre-filter only.
    let candidates: Vec<EntityLocation> = sqlx::query_as::<_, EntityLocation>(
        "SELECT id, entity_id, location_type_id, address, latitude, longitude, timezone, \
         valid_from, valid_until, source_fact_id, created_at \
         FROM entity_locations WHERE entity_id = ? AND location_type_id = ? \
           AND (valid_from IS NULL OR ? IS NULL OR valid_from < ?) \
           AND (? IS NULL OR valid_until IS NULL OR ? < valid_until) \
         ORDER BY created_at, id",
    )
    .bind(entity_id)
    .bind(location_type_id)
    .bind(valid_until)
    .bind(valid_until)
    .bind(valid_from)
    .bind(valid_from)
    .fetch_all(pool)
    .await?;
    let mut tx = pool.begin().await?;
    if let Some(existing) = candidates.iter().find(|row| {
        same_place(
            row.address.as_deref(),
            row.latitude,
            row.longitude,
            address,
            latitude,
            longitude,
        ) && ranges_overlap(row.valid_from, row.valid_until, valid_from, valid_until)
    }) {
        let (merged_from, merged_until) = merged_bounds(
            existing.valid_from,
            existing.valid_until,
            valid_from,
            valid_until,
        );
        let record = sqlx::query_as::<_, EntityLocation>(
            "UPDATE entity_locations \
             SET address = COALESCE(address, ?), \
                 latitude = COALESCE(latitude, ?), \
                 longitude = COALESCE(longitude, ?), \
                 timezone = COALESCE(timezone, ?), \
                 valid_from = ?, valid_until = ? \
             WHERE id = ? \
             RETURNING id, entity_id, location_type_id, address, latitude, longitude, timezone, valid_from, valid_until, source_fact_id, created_at",
        )
        .bind(address)
        .bind(latitude)
        .bind(longitude)
        .bind(timezone)
        .bind(merged_from)
        .bind(merged_until)
        .bind(existing.id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(record);
    }

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

/// Radius (in kilometres) within which two coordinate pairs are considered the
/// same place for re-statement dedup (issue #228).
///
/// Roughly consumer-GPS precision at the same property (~100 m); deliberately
/// larger than a single GPS fix's noise so repeated fixes at one place merge,
/// while genuinely different places (typically hundreds of metres apart) stay
/// distinct.
const SAME_PLACE_RADIUS_KM: f64 = 0.1;

/// Whether two location shapes describe the same place (issue #228).
///
/// A shared attribute that *disagrees* is a veto: different addresses (or
/// coordinates far apart) mean different places even when the other attribute
/// alone would suggest a match — the move semantics of a different address are
/// preserved even if a geocoder returns nearby points for both, and a shared
/// address string is not enough to override coordinates hundreds of metres
/// apart (the same street name can exist in different places). Otherwise the
/// strongest shared *agreement* decides: both addresses present and equal, or
/// both coordinate pairs within [`SAME_PLACE_RADIUS_KM`] (tolerating GPS noise
/// and geocoder drift for the same property). Rows that share no attribute
/// (e.g. one address-only, one coords-only) cannot be linked and are treated
/// as different places.
fn same_place(
    existing_address: Option<&str>,
    existing_latitude: Option<f64>,
    existing_longitude: Option<f64>,
    incoming_address: Option<&str>,
    incoming_latitude: Option<f64>,
    incoming_longitude: Option<f64>,
) -> bool {
    let addresses_agree = match (existing_address, incoming_address) {
        (Some(a), Some(b)) => Some(a == b),
        _ => None,
    };
    let coords_agree = match (
        existing_latitude,
        existing_longitude,
        incoming_latitude,
        incoming_longitude,
    ) {
        (Some(lat1), Some(lon1), Some(lat2), Some(lon2)) => {
            Some(crate::geo::haversine_km(lat1, lon1, lat2, lon2) < SAME_PLACE_RADIUS_KM)
        }
        _ => None,
    };
    if addresses_agree == Some(false) || coords_agree == Some(false) {
        return false;
    }
    addresses_agree == Some(true) || coords_agree == Some(true)
}

/// Merge a re-statement's temporal bounds into the existing row's, producing
/// the interval union (issue #228).
///
/// The merged row starts at the earliest definite `valid_from` when both
/// statements have a start, and ends at the latest definite `valid_until`;
/// either side becomes open-ended when any statement is open-ended on that
/// side (`None` is an unbounded bound, so a start-less statement widens the
/// union to an unbounded start), so a same-place re-statement never closes an
/// open-ended row — the open "currently lives there" claim wins.
fn merged_bounds(
    existing_from: Option<DateTime<Utc>>,
    existing_until: Option<DateTime<Utc>>,
    incoming_from: Option<DateTime<Utc>>,
    incoming_until: Option<DateTime<Utc>>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let valid_from = match (existing_from, incoming_from) {
        (Some(a), Some(b)) => Some(a.min(b)),
        _ => None,
    };
    let valid_until = match (existing_until, incoming_until) {
        (Some(a), Some(b)) => Some(a.max(b)),
        _ => None,
    };
    (valid_from, valid_until)
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

/// Persisted location-overlay shape for a pending sensitive fact, used to
/// rebuild the `entity_locations` row on confirmation (issue #226).
///
/// Only the shape fields are stored: `entity_id` and the temporal bounds come
/// from the confirmed fact, and `fact_id` from the row key. The row is
/// consumed by [`delete_pending_location_meta`] once the overlay is rebuilt;
/// rejecting the fact hard-deletes the row via `ON DELETE CASCADE`.
#[derive(Debug, sqlx::FromRow)]
pub struct PendingLocationMeta {
    pub location_type_id: i16,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timezone: Option<String>,
}

/// Persist the location-overlay shape for a pending sensitive fact.
///
/// Idempotent (`ON CONFLICT DO UPDATE`) so re-extraction of the same pending
/// fact refreshes the shape rather than failing.
#[allow(clippy::too_many_arguments)]
pub async fn insert_pending_location_meta(
    pool: &SqlitePool,
    fact_id: i32,
    location_type_id: i16,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
) -> Result<(), KnowledgeError> {
    let mut tx = pool.begin().await?;
    insert_pending_location_meta_in_tx(
        &mut tx,
        fact_id,
        location_type_id,
        address,
        latitude,
        longitude,
        timezone,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Persist the location-overlay shape for a pending sensitive fact inside an
/// existing transaction.
///
/// The sensitive-fact insert and its overlay shape commit atomically (issue
/// #226): a confirmable fact must never exist without the shape confirmation
/// needs to rebuild its `entity_locations` row. Idempotent (`ON CONFLICT DO
/// UPDATE`) so re-extraction of the same pending fact refreshes the shape
/// rather than failing.
#[allow(clippy::too_many_arguments)]
pub async fn insert_pending_location_meta_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fact_id: i32,
    location_type_id: i16,
    address: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<&str>,
) -> Result<(), KnowledgeError> {
    sqlx::query(
        "INSERT INTO pending_location_meta \
         (fact_id, location_type_id, address, latitude, longitude, timezone) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(fact_id) DO UPDATE SET \
          location_type_id = excluded.location_type_id, \
          address = excluded.address, \
          latitude = excluded.latitude, \
          longitude = excluded.longitude, \
          timezone = excluded.timezone",
    )
    .bind(fact_id)
    .bind(location_type_id)
    .bind(address)
    .bind(latitude)
    .bind(longitude)
    .bind(timezone)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Read the persisted location-overlay shape for a pending sensitive fact, if
/// any.
///
/// Returns `None` for pending facts that carried no location overlay (or that
/// predate the `pending_location_meta` table); callers then skip the overlay
/// rebuild entirely.
pub async fn get_pending_location_meta(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Option<PendingLocationMeta>, KnowledgeError> {
    let row = sqlx::query_as::<_, PendingLocationMeta>(
        "SELECT location_type_id, address, latitude, longitude, timezone \
         FROM pending_location_meta WHERE fact_id = ?",
    )
    .bind(fact_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Remove the persisted location-overlay shape once the overlay has been
/// rebuilt on confirmation.
///
/// Call only after the overlay write has succeeded: on a failed write the row
/// must be retained so the rebuild can be retried. Rejecting a fact
/// hard-deletes the row via `ON DELETE CASCADE`, so this is only needed for
/// the confirm path.
pub async fn delete_pending_location_meta(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<(), KnowledgeError> {
    sqlx::query("DELETE FROM pending_location_meta WHERE fact_id = ?")
        .bind(fact_id)
        .execute(pool)
        .await?;
    Ok(())
}
