CREATE TABLE fact_dependencies (
    parent_fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE RESTRICT,
    child_fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE RESTRICT,
    relation_type_id INTEGER NOT NULL REFERENCES relation_types(id),
    PRIMARY KEY (parent_fact_id, child_fact_id, relation_type_id)
);

CREATE INDEX idx_fact_dependencies_child ON fact_dependencies(child_fact_id);
