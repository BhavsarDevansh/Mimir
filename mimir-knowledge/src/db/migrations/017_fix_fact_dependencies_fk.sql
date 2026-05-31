-- no-transaction
PRAGMA foreign_keys = OFF;

-- 1. Create new table with RESTRICT FKs
CREATE TABLE fact_dependencies_new (
    parent_fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE RESTRICT,
    child_fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE RESTRICT,
    relation_type_id INTEGER NOT NULL REFERENCES relation_types(id),
    PRIMARY KEY (parent_fact_id, child_fact_id, relation_type_id)
);

-- 2. Copy data
INSERT INTO fact_dependencies_new (parent_fact_id, child_fact_id, relation_type_id)
SELECT parent_fact_id, child_fact_id, relation_type_id
FROM fact_dependencies;

-- 3. Drop old table
DROP TABLE fact_dependencies;

-- 4. Rename new table to final name
ALTER TABLE fact_dependencies_new RENAME TO fact_dependencies;

-- 5. Recreate index
CREATE INDEX idx_fact_dependencies_child ON fact_dependencies(child_fact_id);

PRAGMA foreign_keys = ON;
