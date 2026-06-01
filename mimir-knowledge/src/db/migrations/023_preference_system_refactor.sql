-- Preference system refactor (Issue #53)
-- Breaking change: drops old preferences / preference_sources, recreates them
-- with normalized context, source_fact_id NOT NULL, and contextual lookup support.

-- ============================================================================
-- 1. Drop old tables
-- ============================================================================

DROP TABLE IF EXISTS preference_sources;
DROP TABLE IF EXISTS preferences;

-- ============================================================================
-- 2. Re-seed lookup tables
-- ============================================================================

DELETE FROM preference_categories;
INSERT INTO preference_categories (id, name) VALUES
    (1, 'CalendarBehavior'),
    (2, 'NotificationStyle'),
    (3, 'FoodPreference'),
    (4, 'TravelPreference'),
    (5, 'WorkStyle'),
    (6, 'CommunicationPreference'),
    (7, 'General');

DELETE FROM preference_source_types;
INSERT INTO preference_source_types (id, name) VALUES
    (1, 'Interaction'),
    (2, 'Fact'),
    (3, 'UserEdit');

-- ============================================================================
-- 3. Add HasPreference predicate if missing
-- ============================================================================

INSERT OR IGNORE INTO predicates (id, name, description) VALUES
    (11, 'has_preference', 'Subject has a preference');

-- ============================================================================
-- 4. Predicate constraints for HasPreference
-- ============================================================================

INSERT OR IGNORE INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (11, 1, 5), -- Person -> Concept
    (11, 6, 5); -- Organization -> Concept

-- ============================================================================
-- 5. New preferences table
-- ============================================================================

CREATE TABLE preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id INTEGER REFERENCES entities(id),
    category_id INTEGER NOT NULL REFERENCES preference_categories(id),
    key TEXT NOT NULL,
    value TEXT NOT NULL,            -- scalar string/bool/number as text
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    overridden_by_user BOOLEAN NOT NULL DEFAULT FALSE,
    source_fact_id INTEGER NOT NULL REFERENCES facts(id),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_preferences_entity ON preferences(entity_id);
CREATE INDEX idx_preferences_category ON preferences(category_id);
CREATE INDEX idx_preferences_key ON preferences(key);

-- ============================================================================
-- 6. Normalized context table (no JSON)
-- ============================================================================

CREATE TABLE preference_contexts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    preference_id INTEGER NOT NULL REFERENCES preferences(id) ON DELETE CASCADE,
    context_key TEXT NOT NULL,
    context_value TEXT NOT NULL,
    UNIQUE(preference_id, context_key)
);

CREATE INDEX idx_preference_contexts_preference ON preference_contexts(preference_id);

-- ============================================================================
-- 7. New preference_sources table
-- ============================================================================

CREATE TABLE preference_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    preference_id INTEGER NOT NULL REFERENCES preferences(id) ON DELETE CASCADE,
    source_type_id INTEGER NOT NULL REFERENCES preference_source_types(id),
    source_id TEXT NOT NULL,
    extracted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(preference_id, source_type_id, source_id)
);

CREATE INDEX idx_preference_sources_preference ON preference_sources(preference_id);

-- ============================================================================
-- 8. Preference audit log
-- ============================================================================

CREATE TABLE preference_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    preference_id INTEGER NOT NULL,
    change_type_id INTEGER NOT NULL REFERENCES change_types(id),
    old_value TEXT,
    new_value TEXT,
    changed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    changed_by_id INTEGER REFERENCES changed_by_types(id),
    reason TEXT
);

CREATE INDEX idx_preference_audit_log_preference ON preference_audit_log(preference_id);
