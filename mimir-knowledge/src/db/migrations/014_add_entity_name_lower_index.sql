-- Optimise exact-duplicate detection by indexing the lower-cased entity name.
CREATE INDEX idx_entities_name_lower ON entities(LOWER(name));
