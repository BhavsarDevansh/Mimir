-- ============================================================================
-- 052: Category memory buckets (data-driven memory classification)
-- ============================================================================
-- Memory bucketing previously duplicated the taxonomy as hard-coded Rust ID
-- ranges (queries/memory/ranking.rs). This migration makes the bucket a data
-- property of each category: the `memory_buckets` lookup table and the new
-- `categories.memory_bucket_id` column, backfilled to mirror the taxonomy
-- seeded in migration 031.
--
-- Bucket ids are ordered by memory priority (Identity > Upcoming >
-- Relationships > Preferences > General): a fact tagged with several
-- categories resolves to the bucket with the lowest id.

CREATE TABLE memory_buckets (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO memory_buckets (id, name) VALUES
(1, 'Identity'),
(2, 'Upcoming'),
(3, 'Relationships'),
(4, 'Preferences'),
(5, 'General');

ALTER TABLE categories ADD COLUMN memory_bucket_id INTEGER REFERENCES memory_buckets(id);

CREATE INDEX idx_categories_memory_bucket ON categories(memory_bucket_id);

-- Backfill the taxonomy seeded in migration 031, preserving the long-standing
-- classification: identity 100-199, upcoming 900-999, relationships 400-499
-- (including 460 Social Preferences and 480 Communication Preferences, which
-- sit inside the relationships domain), preferences 300-399 plus the
-- preference-ish outliers outside those ranges (570, 670, 680, 690, 830, 870),
-- everything else General.
UPDATE categories SET memory_bucket_id = 1 WHERE id BETWEEN 100 AND 199;
UPDATE categories SET memory_bucket_id = 2 WHERE id BETWEEN 900 AND 999;
UPDATE categories SET memory_bucket_id = 3 WHERE id BETWEEN 400 AND 499;
UPDATE categories SET memory_bucket_id = 4
    WHERE id BETWEEN 300 AND 399 OR id IN (570, 670, 680, 690, 830, 870);
UPDATE categories SET memory_bucket_id = 5 WHERE memory_bucket_id IS NULL;
