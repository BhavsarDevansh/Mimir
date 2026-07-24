//! Entity location model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A location associated with an entity.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct EntityLocation {
    pub id: i32,
    pub entity_id: i32,
    pub location_type_id: i16,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timezone: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    /// Fact that produced this location overlay, when the location was
    /// derived through `normalize_and_insert`. Nullable so a directly-seeded
    /// location (no originating fact) is allowed. `ON DELETE SET NULL` keeps
    /// the location when its source fact is forgotten.
    pub source_fact_id: Option<i32>,
    pub created_at: DateTime<Utc>,
}

/// A location plus its great-circle distance from a query point
/// (Phase 3 S4 / issue #194).
///
/// Returned by [`crate::KnowledgeGraph::find_nearby`]: the SQL layer performs a
/// coarse bounding-box pre-filter, then this distance is computed exactly in
/// Rust (Haversine) and used to drop edge-of-box points outside the radius and
/// to sort the survivors nearest-first. Exposing `distance_km` (rather than
/// only the sort order of `EntityLocation`) makes the result directly
/// testable and useful to callers without re-deriving the distance.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NearbyLocation {
    /// The location row.
    pub location: EntityLocation,
    /// Exact great-circle distance from the query point, in kilometres.
    pub distance_km: f64,
}
