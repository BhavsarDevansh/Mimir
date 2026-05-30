CREATE TABLE preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id INTEGER REFERENCES entities(id),
    category_id INTEGER NOT NULL REFERENCES preference_categories(id),
    key TEXT NOT NULL,
    value TEXT NOT NULL, -- JSON
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    overridden_by_user BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(entity_id, category_id, key)
);

CREATE TABLE preference_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    preference_id INTEGER NOT NULL REFERENCES preferences(id) ON DELETE CASCADE,
    source_type_id INTEGER NOT NULL REFERENCES preference_source_types(id),
    interaction_id TEXT,
    extracted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_preferences_category ON preferences(category_id);
CREATE INDEX idx_preference_sources_preference ON preference_sources(preference_id);
