-- 044: entity_locations provenance link (Phase 3 S3 / issue #193).
--
-- The `entity_locations` table was created in Phase 2 (migration 004) as a
-- stub with no link back to the fact that produced a location. Issue #193
-- wires locations into `normalize_and_insert` as an overlay on a fact (the
-- same pattern as `events.fact_id`), so a location row must trace to its
-- originating fact for provenance and `forget` cascade tracing.
--
-- `source_fact_id` is nullable: a location may be seeded independently of a
-- fact (e.g. a direct edit via the facade), and the `ON DELETE SET NULL`
-- policy keeps the location when its source fact is forgotten rather than
-- orphaning the entity's address history. The table is stub-only (no rows in
-- any existing DB), so a plain `ADD COLUMN` is safe and lossless; a table
-- rebuild is unnecessary.

ALTER TABLE entity_locations
    ADD COLUMN source_fact_id INTEGER REFERENCES facts(id) ON DELETE SET NULL;

CREATE INDEX idx_entity_locations_source_fact ON entity_locations(source_fact_id);
