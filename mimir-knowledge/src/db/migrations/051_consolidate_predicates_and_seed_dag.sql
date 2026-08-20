-- no-transaction
-- ============================================================================
-- 051: Consolidate redundant predicates + seed the relationship-type DAG (#403)
-- ============================================================================
-- Issue #403: the seeded predicate vocabulary overlaps and the relationship
-- type DAG is dormant. This migration:
--
--   1. Consolidates residence: `based_in` + `lived_in` → `resides_in`. Moves
--      are already modelled by `valid_from`/`valid_until` and supersession,
--      so current-vs-previous residence is one relation with temporal bounds.
--   2. Consolidates containment: `is_in` → `located_in` (both express physical
--      containment; the transitivity rule now keys on `located_in`).
--   3. Seeds abstract ontology parents (`residence`, `employment`, `education`,
--      `containment`) so `kg_query --include-subtree` expresses real
--      generalisation. Parents are query-only vocabulary: they are excluded
--      from the Rust `CANONICAL_PREDICATES` allow-list and the strict resolver
--      rejects them as fact predicates.
--
-- Name-keyed throughout (like migration 050): a real database may hold
-- auto-created rows at arbitrary ids, so ids are looked up by name, never
-- assumed. The old names survive as aliases of the consolidated verbs, so
-- existing callers and stored queries keep resolving. Foreign-key
-- enforcement stays ON (the app enables it on every connection): the final
-- DELETEs cascade any alias/constraint/hierarchy rows that were not
-- explicitly repointed, so no orphaned rows can survive.

-- 1. Residence consolidation -------------------------------------------------
-- 1a. Merge any pre-existing `resides_in` row into `based_in` before the
--     rename. A real database may hold an auto-created `resides_in` row with
--     facts from the pre-036 era (migration 050 deliberately preserves
--     auto-created types that have facts, deferring repointing to issue
--     #403); without the merge the rename below would collide with that row
--     on the UNIQUE name constraint and fail the whole migration. Every step
--     is guarded by `EXISTS based_in` so it no-ops when only the
--     auto-created row exists (that row then becomes the canonical verb).
UPDATE facts
SET relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'based_in')
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in');

INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT (SELECT id FROM relationship_types WHERE name = 'based_in'), allowed_subject_type_id, allowed_object_type_id
FROM relationship_constraints
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in');
DELETE FROM relationship_constraints
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in');

INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT (SELECT id FROM relationship_types WHERE name = 'based_in'), parent_id
FROM relationship_type_hierarchy
WHERE child_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in');
DELETE FROM relationship_type_hierarchy
WHERE child_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in');

INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT alias, (SELECT id FROM relationship_types WHERE name = 'based_in')
FROM relationship_type_aliases
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in')
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

DELETE FROM relationship_types WHERE name = 'resides_in'
  AND EXISTS (SELECT 1 FROM relationship_types WHERE name = 'based_in');

-- 1b. Rename based_in → resides_in in place (id preserved).
UPDATE relationship_types SET name = 'resides_in' WHERE name = 'based_in';

-- 1c. Defensive: if based_in was missing, create resides_in fresh. If only an
--     auto-created resides_in exists, it keeps its id but gains the canonical
--     description so the seeded-description contract holds on upgraded DBs.
INSERT OR IGNORE INTO relationship_types (name, description) VALUES
    ('resides_in', 'Subject currently or previously resides in a location');
UPDATE relationship_types SET description = 'Subject currently or previously resides in a location'
WHERE name = 'resides_in' AND description LIKE 'Auto-created relationship_type: %';

-- 1d. Repoint lived_in facts onto resides_in.
UPDATE facts
SET relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'resides_in')
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'lived_in');

-- 1e. Move lived_in constraints and hierarchy edges onto resides_in.
INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT (SELECT id FROM relationship_types WHERE name = 'resides_in'), allowed_subject_type_id, allowed_object_type_id
FROM relationship_constraints
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'lived_in');
DELETE FROM relationship_constraints
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'lived_in');

INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT (SELECT id FROM relationship_types WHERE name = 'resides_in'), parent_id
FROM relationship_type_hierarchy
WHERE child_id = (SELECT id FROM relationship_types WHERE name = 'lived_in');
DELETE FROM relationship_type_hierarchy
WHERE child_id = (SELECT id FROM relationship_types WHERE name = 'lived_in');

-- 1f. Repoint every lived_in alias (self-alias + legacy synonyms such as
--     `previously_lived_in` / `former_city`) onto resides_in, then drop the
--     old row (FK cascade removes any stragglers).
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT alias, (SELECT id FROM relationship_types WHERE name = 'resides_in')
FROM relationship_type_aliases
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'lived_in')
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;
DELETE FROM relationship_types WHERE name = 'lived_in';

-- 2. Containment consolidation ------------------------------------------------
-- 2a. Repoint is_in facts onto located_in.
UPDATE facts
SET relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'located_in')
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'is_in');

-- 2b. Merge is_in constraints into located_in (union; duplicates ignored).
INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT (SELECT id FROM relationship_types WHERE name = 'located_in'), allowed_subject_type_id, allowed_object_type_id
FROM relationship_constraints
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'is_in');
DELETE FROM relationship_constraints
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'is_in');

-- 2c. Move is_in hierarchy edges onto located_in.
INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT (SELECT id FROM relationship_types WHERE name = 'located_in'), parent_id
FROM relationship_type_hierarchy
WHERE child_id = (SELECT id FROM relationship_types WHERE name = 'is_in');
DELETE FROM relationship_type_hierarchy
WHERE child_id = (SELECT id FROM relationship_types WHERE name = 'is_in');

-- 2d. Repoint every is_in alias onto located_in, then drop the old row.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT alias, (SELECT id FROM relationship_types WHERE name = 'located_in')
FROM relationship_type_aliases
WHERE relationship_type_id = (SELECT id FROM relationship_types WHERE name = 'is_in')
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;
DELETE FROM relationship_types WHERE name = 'is_in';

-- 3. Seed abstract ontology parents + DAG edges ------------------------------
-- 3a. Parents (name-keyed UPSERT; fresh ids in a clean database).
INSERT INTO relationship_types (name, description) VALUES
    ('residence', 'Abstract ontology parent for subtree queries; not a fact predicate'),
    ('employment', 'Abstract ontology parent for subtree queries; not a fact predicate'),
    ('education', 'Abstract ontology parent for subtree queries; not a fact predicate'),
    ('containment', 'Abstract ontology parent for subtree queries; not a fact predicate')
ON CONFLICT(name) DO UPDATE SET description = excluded.description;

-- 3b. Self-aliases so the alias table remains the single source of truth.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types WHERE name IN ('residence', 'employment', 'education', 'containment')
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

-- 3c. DAG edges: employment → works_at/works_as/job_title; education →
--     studied/studied_at/completed_degree/educational_status; residence →
--     resides_in; containment → located_in.
INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT c.id, p.id FROM relationship_types c JOIN relationship_types p ON p.name = 'employment'
WHERE c.name IN ('works_at', 'works_as', 'job_title');
INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT c.id, p.id FROM relationship_types c JOIN relationship_types p ON p.name = 'education'
WHERE c.name IN ('studied', 'studied_at', 'completed_degree', 'educational_status');
INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT c.id, p.id FROM relationship_types c JOIN relationship_types p ON p.name = 'residence'
WHERE c.name = 'resides_in';
INSERT OR IGNORE INTO relationship_type_hierarchy (child_id, parent_id)
SELECT c.id, p.id FROM relationship_types c JOIN relationship_types p ON p.name = 'containment'
WHERE c.name = 'located_in';
