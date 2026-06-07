-- ============================================================================
-- 033: Default memory priority per relationship type for auto-assignment
-- ============================================================================
-- no-transaction
PRAGMA foreign_keys = OFF;

-- SQLite cannot ALTER TABLE ADD COLUMN with REFERENCES + non-NULL default.
-- Recreate relationship_types with the new column.

CREATE TABLE relationship_types_new (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    sensitive BOOLEAN NOT NULL DEFAULT FALSE,
    default_memory_priority_id INTEGER NOT NULL DEFAULT 3
        REFERENCES memory_priorities(id)
);

INSERT INTO relationship_types_new (id, name, description, sensitive, default_memory_priority_id)
SELECT id, name, description, COALESCE(sensitive, FALSE), 3
FROM relationship_types;

-- Seed defaults for known relationship types.
UPDATE relationship_types_new SET default_memory_priority_id = 1 WHERE name IN ('has_partner', 'has_parent', 'born_on', 'died_on');
UPDATE relationship_types_new SET default_memory_priority_id = 2 WHERE name IN ('works_as', 'located_in', 'is_in', 'owns');
UPDATE relationship_types_new SET default_memory_priority_id = 3 WHERE name IN ('visited', 'created_on');
UPDATE relationship_types_new SET default_memory_priority_id = 4 WHERE name IN ('rejected_action');

DROP TABLE relationship_types;
ALTER TABLE relationship_types_new RENAME TO relationship_types;

-- Recreate relationship_constraints FK reference.
CREATE TABLE relationship_constraints_new (
    relationship_type_id INTEGER NOT NULL REFERENCES relationship_types(id) ON DELETE CASCADE,
    allowed_subject_type_id INTEGER NOT NULL REFERENCES entity_types(id),
    allowed_object_type_id INTEGER NOT NULL REFERENCES entity_types(id),
    PRIMARY KEY (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
);

INSERT INTO relationship_constraints_new
SELECT * FROM relationship_constraints;

DROP TABLE relationship_constraints;
ALTER TABLE relationship_constraints_new RENAME TO relationship_constraints;

PRAGMA foreign_keys = ON;
