-- no transaction
PRAGMA foreign_keys = OFF;

-- 1. Rename old table
ALTER TABLE facts RENAME TO facts_old;

-- 2. Create new table with predicate_id FK
CREATE TABLE facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id INTEGER NOT NULL REFERENCES entities(id),
    predicate_id INTEGER NOT NULL REFERENCES predicates(id),
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

-- 3. Copy data, converting predicate strings to predicate_id via predicates lookup.
--    Unmatched predicates fall back to id=1 (IsIn).
INSERT INTO facts (
    id, subject_id, predicate_id, object_id, object_literal,
    valid_from, valid_until, confidence, fact_status_id, inferred,
    created_at, updated_at
)
SELECT
    f.id,
    f.subject_id,
    COALESCE(p.id, 1),
    f.object_id,
    f.object_literal,
    f.valid_from,
    f.valid_until,
    f.confidence,
    f.fact_status_id,
    f.inferred,
    f.created_at,
    f.updated_at
FROM facts_old f
LEFT JOIN predicates p ON p.name = f.predicate;

-- 4. Drop old table
DROP TABLE facts_old;

-- 5. Recreate indexes
CREATE INDEX idx_facts_subject ON facts(subject_id);
CREATE INDEX idx_facts_object ON facts(object_id);
CREATE INDEX idx_facts_predicate ON facts(predicate_id);
CREATE INDEX idx_facts_status ON facts(fact_status_id);
CREATE INDEX idx_facts_temporal ON facts(valid_from, valid_until);

PRAGMA foreign_keys = ON;
