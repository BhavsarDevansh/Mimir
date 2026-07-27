-- 047: enforce one `Geographic` location row per place entity (Phase 3 C2 / #196).
--
-- `ensure_place_coordinates` keeps a single `Geographic` row per place so
-- repeated photos at the same place update coordinates in place rather than
-- piling up rows (which would also pollute `find_nearby`'s validity-agnostic
-- spatial scan). That single-row invariant was previously guaranteed only by a
-- code-level "single serial overlay worker" convention: the read-then-write
-- was race-free because the worker drains its queue one job at a time. If the
-- worker is ever parallelised (e.g. to keep up with geocode rate limits on
-- large photo batches), the read-then-write would race and silently produce
-- duplicate `Geographic` rows with no test catching it.
--
-- This migration moves the invariant to the schema: a partial unique index on
-- `entity_id` scoped to `location_type_id = 6` (Geographic). It is *partial*
-- on purpose — `Visited` / `Home` / `Work` / `Origin` / `Current` rows are not
-- unique per `(entity_id, location_type_id)` (a person legitimately visits many
-- places, each a separate `Visited` row), so a full unique index would break
-- those. Only the timeless `Geographic` place-anchor row is unique per place.
--
-- A `Geographic`-only partial unique index has zero effect on the other
-- location types. `ensure_place_coordinates` now uses `INSERT ... ON CONFLICT
-- DO UPDATE` against this index, so the upsert is atomic and the serial-worker
-- convention becomes a performance optimisation rather than a correctness
-- requirement.
--
-- Defensive dedup first: if any duplicate `Geographic` rows already exist
-- (shouldn't happen for fresh databases, but keeps the migration idempotent on
-- a partially-migrated DB), keep the newest row per entity and delete the
-- rest before the unique index is created.

DELETE FROM entity_locations
WHERE id NOT IN (
    SELECT MAX(id)
    FROM entity_locations
    WHERE location_type_id = 6
    GROUP BY entity_id
)
AND location_type_id = 6;

CREATE UNIQUE INDEX idx_entity_locations_geographic_unique
    ON entity_locations(entity_id)
    WHERE location_type_id = 6;
