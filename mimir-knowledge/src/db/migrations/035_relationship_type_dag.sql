-- no-transaction
-- ============================================================================
-- 035: Relationship type DAG schema: hierarchy + aliases
-- ============================================================================
PRAGMA foreign_keys = OFF;

-- ============================================================================
-- 1. Relationship type hierarchy: directed acyclic graph, multiple parents OK
-- ============================================================================
CREATE TABLE relationship_type_hierarchy (
    child_id INTEGER NOT NULL REFERENCES relationship_types(id) ON DELETE CASCADE,
    parent_id INTEGER NOT NULL REFERENCES relationship_types(id) ON DELETE CASCADE,
    PRIMARY KEY (child_id, parent_id),
    CHECK (child_id != parent_id)
);

CREATE INDEX idx_relationship_type_hierarchy_parent ON relationship_type_hierarchy(parent_id);

-- ============================================================================
-- 2. Relationship type aliases: English synonyms, globally unique
-- ============================================================================
CREATE TABLE relationship_type_aliases (
    alias TEXT NOT NULL PRIMARY KEY,
    relationship_type_id INTEGER NOT NULL REFERENCES relationship_types(id) ON DELETE CASCADE
);

CREATE INDEX idx_relationship_type_aliases_type ON relationship_type_aliases(relationship_type_id);

PRAGMA foreign_keys = ON;
