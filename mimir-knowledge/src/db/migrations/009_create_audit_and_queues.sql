CREATE TABLE fact_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    action TEXT NOT NULL, -- INSERT, UPDATE, DELETE, STATUS_CHANGE
    old_value TEXT, -- JSON of previous fact state
    new_value TEXT, -- JSON of new fact state
    performed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    performer TEXT -- 'system', 'user', 'inference_engine', connector_id
);

CREATE INDEX idx_fact_audit_log_fact ON fact_audit_log(fact_id);
CREATE INDEX idx_fact_audit_log_performed_at ON fact_audit_log(performed_at);

CREATE TABLE dedup_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    fact_b_id INTEGER REFERENCES facts(id) ON DELETE CASCADE,
    status_id INTEGER NOT NULL DEFAULT 1 REFERENCES dedup_status_types(id),
    queued_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    processed_at TIMESTAMP
);

CREATE INDEX idx_dedup_queue_fact_a ON dedup_queue(fact_id);
CREATE INDEX idx_dedup_queue_fact_b ON dedup_queue(fact_b_id);
CREATE INDEX idx_dedup_queue_processed ON dedup_queue(processed_at);

CREATE TABLE entity_merge_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    primary_entity_id INTEGER NOT NULL REFERENCES entities(id),
    duplicate_entity_id INTEGER NOT NULL REFERENCES entities(id),
    status_id INTEGER NOT NULL DEFAULT 1 REFERENCES merge_workflow_types(id),
    resolution_id INTEGER REFERENCES merge_resolution_types(id),
    queued_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    processed_at TIMESTAMP,
    UNIQUE(primary_entity_id, duplicate_entity_id)
);

CREATE INDEX idx_entity_merge_queue_processed ON entity_merge_queue(processed_at);
