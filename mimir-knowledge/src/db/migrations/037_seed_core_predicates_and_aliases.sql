-- no-transaction
-- ============================================================================
-- 037: Seed remaining core relationship predicates + self-aliases (#135)
-- ============================================================================
-- Category-first ontology: predicates are thin canonical verbs; grouping
-- lives in the Dewey `categories` tree (see migration 038). This migration
-- adds the canonical verbs referenced by the extraction prompt and
-- `LIST_PREDICATES` that were not yet seeded by migration 036, plus their
-- self-aliases so the alias table remains the single source of truth for
-- resolution. Idempotent via UPSERT (ON CONFLICT).
PRAGMA foreign_keys = OFF;

-- 1. Canonical verbs (explicit ids 26-31; 1-25 are taken by earlier migrations).
-- UPSERT (not INSERT OR IGNORE) enforces the canonical (id, name) contract: on
-- upgrade a pre-existing row at a reserved id is corrected to the canonical
-- name/description, and a conflicting name elsewhere surfaces a UNIQUE error
-- rather than silently preserving a stale mapping.
INSERT INTO relationship_types (id, name, description) VALUES
    (26, 'studied', 'Subject studied a particular subject or field'),
    (27, 'completed_degree', 'Subject completed an academic degree'),
    (28, 'educational_status', 'Subject''s current educational/enrolment status'),
    (29, 'job_title', 'Subject''s current job title or role label'),
    (30, 'likes', 'Subject likes a particular thing or activity'),
    (31, 'dislikes', 'Subject dislikes a particular thing or activity')
ON CONFLICT(id) DO UPDATE SET
    name = excluded.name,
    description = excluded.description;

-- 2. Self-aliases for the new canonical verbs. UPSERT so a stale alias row is
-- repointed to the canonical id instead of being silently kept.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types WHERE id IN (26, 27, 28, 29, 30, 31)
ON CONFLICT(alias) DO UPDATE SET
    relationship_type_id = excluded.relationship_type_id;

PRAGMA foreign_keys = ON;
