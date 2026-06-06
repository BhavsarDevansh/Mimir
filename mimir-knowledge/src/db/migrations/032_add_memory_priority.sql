-- ============================================================================
-- 032: Memory priority system for fact ranking in memory condensation
-- ============================================================================
-- no-transaction
PRAGMA foreign_keys = OFF;

-- Lookup table for memory priority tiers used by the ranking engine.
CREATE TABLE memory_priorities (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO memory_priorities (id, name) VALUES
    (1, 'Critical'),
    (2, 'High'),
    (3, 'Normal'),
    (4, 'Low');

-- Add memory_priority_id to facts with default Normal (3).
ALTER TABLE facts ADD COLUMN memory_priority_id INTEGER NOT NULL DEFAULT 3
    REFERENCES memory_priorities(id);

CREATE INDEX idx_facts_memory_priority ON facts(memory_priority_id);

PRAGMA foreign_keys = ON;
