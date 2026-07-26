-- 045: composite coordinate index for proximity queries (Phase 3 S4 / #194).
--
-- `find_nearby` pre-filters with `WHERE latitude BETWEEN ? AND ? AND longitude
-- BETWEEN ? AND ?` before an exact Haversine post-filter in Rust. The Phase 2
-- indexes (004) cover `entity_id` and `location_type_id` only, so the bounding
-- box would scan the whole table. This composite index lets SQLite satisfy the
-- two-sided latitude range (the leading, selective column) and then refine on
-- longitude within each matched latitude band.
--
-- Additive `CREATE INDEX`; no data changes, no table rebuild. A regular SQLite
-- b-tree index includes every row (NULLs included); only a partial index with a
-- `WHERE` clause can omit rows. The `WHERE latitude IS NOT NULL AND longitude IS
-- NOT NULL` clause here keeps the index limited to the geocoded rows that
-- `find_nearby`'s `BETWEEN` pre-filter can actually match, skipping the
-- un-geocoded set while shrinking the index.

CREATE INDEX idx_entity_locations_coords
    ON entity_locations(latitude, longitude)
    WHERE latitude IS NOT NULL AND longitude IS NOT NULL;
