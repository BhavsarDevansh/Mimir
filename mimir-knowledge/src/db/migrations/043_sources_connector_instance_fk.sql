-- 043: sources provenance FK migration (issue #180 / Phase 3 F3).
--
-- Migrate `sources.connector_id TEXT` to `connector_instance_id INTEGER
-- REFERENCES connectors(id)`. SQLite cannot change a column type in place, so
-- this is the standard table-rebuild dance. It is lossless for production DBs:
-- no connector instances are registered yet, so every row carries either NULL
-- or '' (the insert paths differ — `queries/source.rs` normalises a missing
-- connector to '', `queries/fact.rs` binds NULL). Both are treated as "no
-- instance" and map to `connector_instance_id IS NULL`.
--
-- `connector_type_id` is retained (denormalised) so the confidence model can
-- read the connector kind without a join, even when `connector_instance_id`
-- is NULL (e.g. legacy rows, or non-connector sources that happen to carry a
-- type).

CREATE TABLE sources_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    source_type_id INTEGER NOT NULL REFERENCES source_types(id),
    connector_instance_id INTEGER REFERENCES connectors(id),
    connector_type_id INTEGER REFERENCES connector_types(id),
    raw_reference TEXT,
    extracted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    extraction_method_id INTEGER REFERENCES extraction_methods(id)
);

-- Copy every row, mapping both NULL and '' on the old connector_id to NULL.
INSERT INTO sources_new (
    id, fact_id, source_type_id, connector_instance_id, connector_type_id,
    raw_reference, extracted_at, extraction_method_id
)
SELECT
    id,
    fact_id,
    source_type_id,
    NULL,  -- no production row carries a real connector instance id yet;
            -- the old text column held only NULL or '' (see header), so
            -- every legacy row maps to connector_instance_id IS NULL.
    connector_type_id,
    raw_reference,
    extracted_at,
    extraction_method_id
FROM sources;

-- Drop the old table and adopt the rebuilt schema.
DROP TABLE sources;
ALTER TABLE sources_new RENAME TO sources;

-- NULL-aware unique index: a missing instance id is treated as the sentinel 0
-- (autoincrement ids start at 1, so 0 never collides with a real instance).
CREATE UNIQUE INDEX idx_sources_unique
    ON sources(fact_id, source_type_id, COALESCE(connector_instance_id, 0), COALESCE(raw_reference, ''));
CREATE INDEX idx_sources_fact ON sources(fact_id);
CREATE INDEX idx_sources_type ON sources(source_type_id);
CREATE INDEX idx_sources_instance ON sources(connector_instance_id);
