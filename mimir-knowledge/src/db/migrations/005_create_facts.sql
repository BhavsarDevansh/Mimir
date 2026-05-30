CREATE TABLE facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id INTEGER NOT NULL REFERENCES entities(id),
    predicate TEXT NOT NULL,
    object_id INTEGER REFERENCES entities(id),
    object_literal TEXT,
    valid_from TIMESTAMP,
    valid_until TIMESTAMP,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    fact_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES fact_statuses(id),
    inferred BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_facts_subject ON facts(subject_id);
CREATE INDEX idx_facts_object ON facts(object_id);
CREATE INDEX idx_facts_predicate ON facts(predicate);
CREATE INDEX idx_facts_status ON facts(fact_status_id);
CREATE INDEX idx_facts_temporal ON facts(valid_from, valid_until);
