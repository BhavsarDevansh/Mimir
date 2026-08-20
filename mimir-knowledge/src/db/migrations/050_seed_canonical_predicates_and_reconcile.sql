-- no-transaction
-- ============================================================================
-- 050: Seed remaining canonical predicates + reconcile auto-created types (#401)
-- ============================================================================
-- Issue #401: the conversational extraction path now enforces a Rust-side
-- canonical predicate allow-list (`CANONICAL_PREDICATES` in
-- `mimir-knowledge/src/graph/predicates.rs`) and rejects unknown predicates
-- instead of auto-creating `relationship_types` rows. This migration seeds the
-- predicates the extraction path legitimately uses that were not yet canonical:
--
--   * `skill` — already in the Rust `LIST_PREDICATES` / `MULTI_VALUED_PREDICATES`
--     allow-lists but never seeded.
--   * `has_appointment` — appointment facts are a first-class concept in the
--     events subsystem (migration 039) and the email connector emits them.
--   * The sensitive predicates migration 029 intended to mark (its UPDATE was a
--     no-op because the rows never existed): allergy, medication, diagnosis,
--     income, salary, password, ssn, social_security_number, bank_account,
--     credit_card, insurance. `condition` is deliberately NOT seeded — it is
--     already an alias of `health_condition` (migration 036).
--
-- Name-keyed UPSERT (not id-keyed like migration 037): a real database may
-- already hold an auto-created row with one of these names at an arbitrary id;
-- keying on `name` canonicalises that row in place instead of colliding with a
-- reserved id. New rows get fresh ids.
--
-- Reconciliation: auto-created relationship types that no fact references are
-- pure vocabulary pollution and are deleted (aliases/constraints/hierarchy
-- cascade). Auto-created types WITH facts are preserved — repointing existing
-- facts onto canonical predicates is the ontology consolidation's job (issue
-- #403), and the strict resolver rejects new facts with them.
PRAGMA foreign_keys = OFF;

-- 1. Canonical predicates (name-keyed UPSERT).
INSERT INTO relationship_types (name, description, sensitive) VALUES
    ('skill', 'Subject has a skill or competency', FALSE),
    ('has_appointment', 'Subject has an appointment or scheduled meeting', FALSE),
    ('allergy', 'Subject has an allergy or allergic reaction', TRUE),
    ('medication', 'Subject takes a medication or treatment', TRUE),
    ('diagnosis', 'Subject has a medical diagnosis', TRUE),
    ('income', 'Subject''s income or earnings', TRUE),
    ('salary', 'Subject''s salary or compensation', TRUE),
    ('password', 'Subject''s password or credential', TRUE),
    ('ssn', 'Subject''s social security number', TRUE),
    ('social_security_number', 'Subject''s social security number', TRUE),
    ('bank_account', 'Subject''s bank account details', TRUE),
    ('credit_card', 'Subject''s credit card details', TRUE),
    ('insurance', 'Subject''s insurance policy or coverage', TRUE)
ON CONFLICT(name) DO UPDATE SET
    description = excluded.description,
    sensitive = excluded.sensitive;

-- 2. Self-aliases for the new canonical predicates. UPSERT so a stale alias
-- row is repointed to the canonical id instead of being silently kept.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types WHERE name IN (
    'skill', 'has_appointment', 'allergy', 'medication', 'diagnosis',
    'income', 'salary', 'password', 'ssn', 'social_security_number',
    'bank_account', 'credit_card', 'insurance'
)
ON CONFLICT(alias) DO UPDATE SET
    relationship_type_id = excluded.relationship_type_id;

-- 3. Reconciliation: drop auto-created types no fact references.
DELETE FROM relationship_types
WHERE description LIKE 'Auto-created relationship_type: %'
  AND id NOT IN (SELECT DISTINCT relationship_type_id FROM facts);

PRAGMA foreign_keys = ON;
