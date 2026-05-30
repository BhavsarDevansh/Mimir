CREATE TABLE sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    source_type_id INTEGER NOT NULL REFERENCES source_types(id),
    connector_id TEXT,
    raw_reference TEXT,
    extracted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    extraction_method TEXT
);

CREATE INDEX idx_sources_fact ON sources(fact_id);
CREATE INDEX idx_sources_type ON sources(source_type_id);
