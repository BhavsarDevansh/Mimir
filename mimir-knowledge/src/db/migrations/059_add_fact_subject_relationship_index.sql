-- 059: Composite fact index for subject/relationship scans (#527).
--
-- Fact lookups start with an equal subject and relationship pair. Migration
-- 028's tool-query index adds unrelated columns after `subject_id`, while the
-- pre-existing single-column indexes can satisfy only one equality. Keep the
-- index limited to the two equalities so SQLite can use it for the dedup
-- self-join without adding write maintenance for columns it does not seek.

CREATE INDEX IF NOT EXISTS idx_facts_subject_relationship
    ON facts(subject_id, relationship_type_id, object_id, confidence DESC);
