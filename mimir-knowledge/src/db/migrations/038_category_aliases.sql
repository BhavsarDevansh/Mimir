-- ============================================================================
-- 038: Category aliases — natural-language lookup into the Dewey taxonomy
-- ============================================================================
-- Categories own grouping/hierarchy and multi-tag precision. This table lets
-- callers resolve a domain word ("hobbies", "education") to a category id,
-- enabling category-subtree retrieval without a predicate hierarchy.
-- Aliases are globally unique (one alias -> one category). Idempotent via IF NOT EXISTS and INSERT OR IGNORE; runs inside a transaction with FK enforcement on.

CREATE TABLE IF NOT EXISTS category_aliases (
    alias TEXT NOT NULL PRIMARY KEY,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_category_aliases_category ON category_aliases(category_id);

-- Seed domain/synonym aliases mapping to existing Dewey category nodes.
-- Six core domains from issue #135 (education, employment, residence,
-- personal, family, identity) map to the best-fit existing category nodes;
-- "personal" is spread across hobbies/leisure and pets rather than a single
-- synthetic top-level node, matching the Dewey design.
INSERT OR IGNORE INTO category_aliases (alias, category_id) VALUES
    ('education', 550),
    ('schooling', 550),
    ('academics', 550),
    ('studies', 550),
    ('employment', 510),
    ('career', 510),
    ('job', 510),
    ('work', 510),
    ('residence', 610),
    ('housing', 610),
    ('hometown', 610),
    ('location', 610),
    ('hobbies', 770),
    ('interests', 770),
    ('leisure', 700),
    ('pastimes', 770),
    ('pets', 440),
    ('animals', 440),
    ('family', 410),
    ('relatives', 410),
    ('kin', 410),
    ('identity', 100),
    ('biography', 100),
    ('profile', 100);

