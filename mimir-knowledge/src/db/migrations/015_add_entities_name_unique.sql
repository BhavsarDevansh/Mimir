-- Enforce case-insensitive name uniqueness at the DB level.
CREATE UNIQUE INDEX idx_entities_name_unique ON entities(LOWER(name));
