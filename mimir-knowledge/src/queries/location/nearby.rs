//! Geographic near-by search over entity locations.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::geo;
use crate::models::entity_location::{EntityLocation, NearbyLocation};

/// Find entity locations within `radius_km` of (`lat`, `lon`), sorted
/// nearest-first (Phase 3 S4 / issue #194).
///
/// Coarse SQLite bounding-box pre-filter (via [`crate::geo::bounding_box`])
/// plus exact Haversine post-filter (via [`crate::geo::haversine_km`]) in
/// Rust; each result carries its exact great-circle `distance_km`. Pass
/// `Some(t)` to restrict to locations whose `valid_from`/`valid_until`
/// window contains `t`; `None` scans all locations regardless of temporal
/// validity.
pub async fn find_nearby(
    pool: &SqlitePool,
    lat: f64,
    lon: f64,
    radius_km: f64,
    at: Option<DateTime<Utc>>,
) -> Result<Vec<NearbyLocation>, KnowledgeError> {
    let (min_lat, max_lat, min_lon, max_lon) = geo::bounding_box(lat, lon, radius_km);

    // Build the SQL once; the temporal clause is appended only when `at` is
    // set. `AssertSqlSafe` marks the dynamically-built string as trusted
    // (no user content is interpolated — all values are bound parameters),
    // matching the existing `get_entity_names` pattern in this module.
    let sql = format!(
        "SELECT id, entity_id, location_type_id, address, latitude, longitude, \
                timezone, valid_from, valid_until, source_fact_id, created_at \
         FROM entity_locations \
         WHERE latitude IS NOT NULL AND longitude IS NOT NULL \
           AND latitude BETWEEN ? AND ? \
           AND longitude BETWEEN ? AND ?{temporal}",
        temporal = if at.is_some() {
            " AND (valid_from IS NULL OR valid_from <= ?) \
             AND (valid_until IS NULL OR valid_until >= ?)"
        } else {
            ""
        }
    );

    let mut query = sqlx::query_as::<_, EntityLocation>(sqlx::AssertSqlSafe(&*sql))
        .bind(min_lat)
        .bind(max_lat)
        .bind(min_lon)
        .bind(max_lon);
    if let Some(t) = at {
        query = query.bind(t).bind(t);
    }

    let candidates = query.fetch_all(pool).await?;

    let mut results: Vec<NearbyLocation> = candidates
        .into_iter()
        .filter_map(|loc| {
            let latitude = loc.latitude?;
            let longitude = loc.longitude?;
            let distance_km = geo::haversine_km(lat, lon, latitude, longitude);
            (distance_km <= radius_km).then_some(NearbyLocation {
                location: loc,
                distance_km,
            })
        })
        .collect();
    results.sort_by(|a, b| a.distance_km.total_cmp(&b.distance_km));
    Ok(results)
}
