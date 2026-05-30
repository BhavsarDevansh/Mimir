CREATE TABLE entity_dates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    date_type_id INTEGER NOT NULL REFERENCES entity_date_types(id),
    date_value TEXT NOT NULL, -- ISO-8601 date or datetime
    recurrence_type_id INTEGER NOT NULL DEFAULT 1 REFERENCES recurrence_types(id),
    custom_label TEXT,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_entity_dates_entity ON entity_dates(entity_id);
CREATE INDEX idx_entity_dates_type ON entity_dates(date_type_id);
