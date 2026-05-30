CREATE TABLE entity_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    location_type_id INTEGER NOT NULL REFERENCES location_types(id),
    address TEXT,
    latitude REAL,
    longitude REAL,
    timezone TEXT,
    valid_from TIMESTAMP,
    valid_until TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_entity_locations_entity ON entity_locations(entity_id);
CREATE INDEX idx_entity_locations_type ON entity_locations(location_type_id);
