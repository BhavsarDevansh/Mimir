-- no transaction
PRAGMA foreign_keys = OFF;

-- 1. Rename old table
ALTER TABLE fact_dependencies RENAME TO fact_dependencies_old;

-- 2. Create new table with RESTRICT FKs
CREATE TABLE fact_dependencies (
    parent_fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE RESTRICT,
    child_fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE RESTRICT,
    relation_type_id INTEGER NOT NULL REFERENCES relation_types(id),
    PRIMARY KEY (parent_fact_id, child_fact_id, relation_type_id)
);

-- 3. Copy data
INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id)
SELECT parent_fact_id, child_fact_id, relation_type_id
FROM fact_dependencies_old;

-- 4. Drop old table
DROP TABLE fact_dependencies_old;

-- 5. Recreate index
CREATE INDEX idx_fact_dependencies_child ON fact_dependencies(child_fact_id);

PRAGMA foreign_keys = ON;
