use crate::graph::KnowledgeGraph;
use crate::*;

use chrono::{DateTime, Utc};

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Entity locations (Phase 2 stubs + Phase 3 S3 / #193 write path)
    // ------------------------------------------------------------------

    /// Insert a location for an entity (direct-seed path; no supersession).
    ///
    /// Opens its own transaction. For location *moves* (closing a prior
    /// open-ended location of the same type) use [`Self::upsert_location`].
    /// `source_fact_id` links the row to the fact that produced it when the
    /// location was derived through `normalize_and_insert`.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_location(
        &self,
        entity_id: i32,
        location_type: models::enums::LocationType,
        address: Option<&str>,
        latitude: Option<f64>,
        longitude: Option<f64>,
        timezone: Option<&str>,
        valid_from: Option<DateTime<Utc>>,
        valid_until: Option<DateTime<Utc>>,
        source_fact_id: Option<i32>,
    ) -> Result<models::entity_location::EntityLocation, KnowledgeError> {
        queries::entity::insert_location(
            &self.pool,
            entity_id,
            location_type as i16,
            address,
            latitude,
            longitude,
            timezone,
            valid_from,
            valid_until,
            source_fact_id,
        )
        .await
    }

    /// Upsert a location for an entity with move/supersession semantics.
    ///
    /// A same-place re-statement (the same address or coordinates as an
    /// existing row of the same `entity_id` + `location_type` whose period
    /// overlaps it) is deduplicated instead: the existing row absorbs the
    /// incoming bounds (interval union) and any shape fields it is missing,
    /// and is returned — no duplicate row is created (issue #228). Otherwise
    /// this closes any still-open location of the same `entity_id` +
    /// `location_type` that began before `valid_from` (sets its
    /// `valid_until = valid_from`), then inserts the new row — modelling a
    /// move such as "home 2020-2023, home 2023-present". The whole operation
    /// is atomic in one transaction. Returns the persisted location.
    ///
    /// Geocoding (filling the missing half of address/coords) is the caller's
    /// responsibility; this method persists exactly what it is given.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_location(
        &self,
        entity_id: i32,
        location_type: models::enums::LocationType,
        address: Option<&str>,
        latitude: Option<f64>,
        longitude: Option<f64>,
        timezone: Option<&str>,
        valid_from: Option<DateTime<Utc>>,
        valid_until: Option<DateTime<Utc>>,
        source_fact_id: Option<i32>,
    ) -> Result<models::entity_location::EntityLocation, KnowledgeError> {
        queries::entity::upsert_location(
            self.pool(),
            entity_id,
            location_type as i16,
            address,
            latitude,
            longitude,
            timezone,
            valid_from,
            valid_until,
            source_fact_id,
        )
        .await
    }

    /// Get locations for an entity.
    pub async fn get_locations(
        &self,
        entity_id: i32,
    ) -> Result<Vec<models::entity_location::EntityLocation>, KnowledgeError> {
        queries::entity::get_locations(&self.pool, entity_id).await
    }

    /// Update a location's mutable fields (address/coords/timezone).
    pub async fn update_location(
        &self,
        id: i32,
        address: Option<&str>,
        latitude: Option<f64>,
        longitude: Option<f64>,
        timezone: Option<&str>,
    ) -> Result<models::entity_location::EntityLocation, KnowledgeError> {
        queries::entity::update_location(&self.pool, id, address, latitude, longitude, timezone)
            .await
    }

    /// Find entity locations within `radius_km` of `(latitude, longitude)`,
    /// sorted nearest-first (Phase 3 S4 / issue #194).
    ///
    /// Coarse SQLite bounding-box pre-filter + exact Haversine post-filter in
    /// Rust (see [`queries::entity::find_nearby`]). Each result carries its
    /// exact great-circle `distance_km`.
    ///
    /// Pass `Some(t)` to restrict to locations whose `valid_from`/`valid_until`
    /// bounds contain `t` (e.g. "where was I living on 2024-06-01"); `None`
    /// for a pure spatial query over all locations, including historical
    /// `Visited`/`Origin` overlays.
    pub async fn find_nearby(
        &self,
        latitude: f64,
        longitude: f64,
        radius_km: f64,
        at: Option<DateTime<Utc>>,
    ) -> Result<Vec<models::entity_location::NearbyLocation>, KnowledgeError> {
        queries::entity::find_nearby(&self.pool, latitude, longitude, radius_km, at).await
    }
}
