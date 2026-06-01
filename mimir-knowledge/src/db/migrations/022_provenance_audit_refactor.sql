-- Provenance audit refactor (Issue #52)
-- Breaking change: remaps source_types to 6 canonical variants,
-- introduces extraction_methods / change_types / changed_by_types lookup tables,
-- and recreates sources + fact_audit_log with typed foreign keys.

-- ============================================================================
-- 1. New lookup tables
-- ============================================================================

CREATE TABLE extraction_methods (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO extraction_methods (id, name) VALUES
    (1, 'llm_extraction'),
    (2, 'structured_parse'),
    (3, 'user_input'),
    (4, 'inference_rule'),
    (5, 'dedup_merge');

CREATE TABLE change_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO change_types (id, name) VALUES
    (1, 'created'),
    (2, 'status_change'),
    (3, 'confidence_change'),
    (4, 'temporal_update'),
    (5, 'source_added'),
    (6, 'forgotten'),
    (7, 'restored');

CREATE TABLE changed_by_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO changed_by_types (id, name) VALUES
    (1, 'user'),
    (2, 'system'),
    (3, 'inference_engine'),
    (4, 'nightly_optimization');

-- ============================================================================
-- 2. Remap source_types to 6 canonical variants
-- ============================================================================

-- Map old source_type_ids in the sources table before we drop old rows.
UPDATE sources SET source_type_id = 2 WHERE source_type_id IN (1, 2, 3, 4, 7); -- Email, Calendar, Photo, Message, Connector -> Connector
UPDATE sources SET source_type_id = 3 WHERE source_type_id = 5;                -- Inference -> Inference
UPDATE sources SET source_type_id = 1 WHERE source_type_id = 6;                -- UserEdit -> UserEdit
UPDATE sources SET source_type_id = 4 WHERE source_type_id = 8;                -- CasualMention -> Interaction
UPDATE sources SET source_type_id = 5 WHERE source_type_id = 9;                -- Import -> Import
UPDATE sources SET source_type_id = 6 WHERE source_type_id = 10;               -- System -> System

DELETE FROM source_types;

INSERT INTO source_types (id, name) VALUES
    (1, 'UserEdit'),
    (2, 'Connector'),
    (3, 'Inference'),
    (4, 'Interaction'),
    (5, 'Import'),
    (6, 'System');

-- ============================================================================
-- 3. Recreate sources with extraction_method_id and UNIQUE constraint
-- ============================================================================

CREATE TABLE sources_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    source_type_id INTEGER NOT NULL REFERENCES source_types(id),
    connector_id TEXT,
    connector_type_id INTEGER REFERENCES connector_types(id),
    raw_reference TEXT,
    extracted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    extraction_method_id INTEGER REFERENCES extraction_methods(id)
);

INSERT INTO sources_new (
    id, fact_id, source_type_id, connector_id, connector_type_id,
    raw_reference, extracted_at, extraction_method_id
)
SELECT
    id,
    fact_id,
    source_type_id,
    connector_id,
    connector_type_id,
    raw_reference,
    extracted_at,
    NULL  -- old extraction_method text has no reliable mapping to IDs
FROM sources;

DROP TABLE sources;
ALTER TABLE sources_new RENAME TO sources;

CREATE UNIQUE INDEX idx_sources_unique
    ON sources(fact_id, source_type_id, connector_id, raw_reference);
CREATE INDEX idx_sources_fact ON sources(fact_id);
CREATE INDEX idx_sources_type ON sources(source_type_id);

-- ============================================================================
-- 4. Recreate fact_audit_log with typed change_type_id / changed_by_id
-- ============================================================================

CREATE TABLE fact_audit_log_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL,
    change_type_id INTEGER NOT NULL REFERENCES change_types(id),
    old_value TEXT,
    new_value TEXT,
    changed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    changed_by_id INTEGER REFERENCES changed_by_types(id),
    reason TEXT
);

INSERT INTO fact_audit_log_new (
    id, fact_id, change_type_id, old_value, new_value,
    changed_at, changed_by_id, reason
)
SELECT
    id,
    fact_id,
    CASE action
        WHEN 'INSERT' THEN 1       -- created
        WHEN 'STATUS_CHANGE' THEN 2
        WHEN 'DELETE' THEN 6       -- forgotten
        ELSE 4                     -- temporal_update for old 'UPDATE' and anything else
    END,
    old_value,
    new_value,
    performed_at,
    CASE performer
        WHEN 'user' THEN 1
        WHEN 'system' THEN 2
        WHEN 'inference_engine' THEN 3
        ELSE 2                     -- default to system for unknown performers
    END,
    NULL
FROM fact_audit_log;

DROP TABLE fact_audit_log;
ALTER TABLE fact_audit_log_new RENAME TO fact_audit_log;

CREATE INDEX idx_fact_audit_log_fact ON fact_audit_log(fact_id);
CREATE INDEX idx_fact_audit_log_changed_at ON fact_audit_log(changed_at);
