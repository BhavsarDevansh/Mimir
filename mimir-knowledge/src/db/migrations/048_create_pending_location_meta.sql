-- 048: Pending location-shape metadata for sensitive facts (issue #226).
--
-- Sensitive "where" facts return `Pending` before the entity-locations overlay
-- block in normalize/process.rs, so the structured NormalizedLocation shape
-- (location type, address, coords, timezone) would otherwise be lost across the
-- confirmation boundary and `confirm_fact` would have nothing to rebuild the
-- `entity_locations` row from. To rebuild the overlay faithfully on
-- confirmation, the location shape computed at extraction time is persisted
-- here, keyed on the pending fact — the same pattern as `pending_event_meta`
-- (migration 041) for the events subsystem. The row is consumed (deleted) when
-- the fact is confirmed and the overlay is rebuilt; on rejection the fact is
-- hard-deleted and the `ON DELETE CASCADE` foreign key removes the metadata
-- automatically, so no orphan location row can be left behind.

CREATE TABLE pending_location_meta (
    fact_id INTEGER PRIMARY KEY REFERENCES facts(id) ON DELETE CASCADE,
    location_type_id INTEGER NOT NULL REFERENCES location_types(id),
    address TEXT,
    latitude REAL,
    longitude REAL,
    timezone TEXT
);
