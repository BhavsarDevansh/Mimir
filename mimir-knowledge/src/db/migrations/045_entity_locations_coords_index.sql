-- 045: composite coordinate index for proximity queries (Phase 3 S4 / #194).
--
-- `find_nearby` pre-filters with `WHERE latitude BETWEEN ? AND ? AND longitude
-- BETWEEN ? AND ?` before an exact Haversine post-filter in Rust. The Phase 2
-- indexes (004) cover `entity_id` and `location_type_id` only, so the bounding
-- box would scan the whole table. This composite index lets SQLite satisfy the
-- two-sided latitude range (the leading, selective column) and then refine on
-- longitude within each matched latitude band.
--
-- Additive `CREATE INDEX`; no data changes, no table rebuild. Locations with
-- NULL coordinates are simply not indexed (SQLite does not index NULLs by
-- default for b-tree indexes), which is exactly the set `find_nearby` must
-- skip anyway.

CREATE INDEX idx_entity_locations_coords
    ON entity_locations(latitude, longitude);
