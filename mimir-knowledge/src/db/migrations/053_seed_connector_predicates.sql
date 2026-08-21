-- no-transaction
-- ============================================================================
-- 053: Seed connector-emitted predicates as canonical + constraints (#412)
-- ============================================================================
-- Issue #412: the calendar connector's `has_event` predicate (and the other
-- deterministic connector predicates) were silently auto-created by
-- `ensure_relationship_type` on first sync because they appeared in no
-- migration seed. This migration seeds every predicate the connectors emit
-- deterministically as canonical vocabulary — with a description, a
-- self-alias, and subject/object constraints — so the rows exist up front,
-- the strict resolver accepts them, and the renderer has a template.
--
-- Name-keyed UPSERT (like migrations 050/051): a real database may already
-- hold an auto-created row with one of these names at an arbitrary id (a
-- calendar that synced before this migration); keying on `name` canonicalises
-- that row in place instead of colliding with a reserved id.
--
-- Constraint pairs mirror the connector emit sites (entity ids from
-- migrations 001/012: 1 Person, 2 Place, 3 Event, 6 Organization). `took_photo`
-- is deliberately left unconstrained: the Photos connector always emits it
-- with a literal object (the photo path), which the write boundary exempts.
--
-- Reconciliation: like migration 050, auto-created relationship types that no
-- fact references are pure vocabulary pollution and are deleted (aliases /
-- constraints / hierarchy cascade via FK enforcement); auto-created types WITH
-- facts are preserved and canonicalised in place by the UPSERT.

-- 1. Canonical predicates (name-keyed UPSERT).
INSERT INTO relationship_types (name, description, sensitive) VALUES
    ('has_event', 'Subject has a scheduled event or engagement', FALSE),
    ('attending', 'Subject is attending an event', FALSE),
    ('took_photo_at', 'Subject took a photo at a location', FALSE),
    ('took_photo', 'Subject took a photo (literal record)', FALSE),
    ('has_flight', 'Subject has a booked flight', FALSE),
    ('departs_from', 'Subject departs from a location', FALSE),
    ('arrives_at', 'Subject arrives at a location', FALSE),
    ('operated_by', 'Subject is operated by an organization', FALSE),
    ('has_booking', 'Subject has a booking or reservation', FALSE),
    ('has_order', 'Subject has an order or purchase', FALSE),
    ('purchased_from', 'Subject was purchased from an organization', FALSE),
    ('has_delivery', 'Subject has a parcel delivery in transit', FALSE),
    ('shipped_by', 'Subject was shipped by an organization', FALSE),
    ('delivered_to', 'Subject is delivered to a location', FALSE),
    ('has_ticket', 'Subject has a ticket', FALSE),
    ('issued_by', 'Subject was issued by an organization', FALSE)
ON CONFLICT(name) DO UPDATE SET
    description = excluded.description,
    sensitive = excluded.sensitive;

-- 2. Self-aliases for the new canonical predicates. UPSERT so a stale alias
-- row is repointed to the canonical id instead of being silently kept.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types WHERE name IN (
    'has_event', 'attending', 'took_photo_at', 'took_photo', 'has_flight',
    'departs_from', 'arrives_at', 'operated_by', 'has_booking', 'has_order',
    'purchased_from', 'has_delivery', 'shipped_by', 'delivered_to',
    'has_ticket', 'issued_by'
)
ON CONFLICT(alias) DO UPDATE SET
    relationship_type_id = excluded.relationship_type_id;

-- 3. Subject/object constraints mirroring the connector emit sites.
INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT id, 1, 3 FROM relationship_types WHERE name IN
    ('has_event', 'attending', 'has_flight', 'has_booking', 'has_order',
     'has_delivery', 'has_ticket');
INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT id, 1, 2 FROM relationship_types WHERE name = 'took_photo_at';
INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT id, 3, 2 FROM relationship_types WHERE name IN
    ('departs_from', 'arrives_at', 'delivered_to');
INSERT OR IGNORE INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT id, 3, 6 FROM relationship_types WHERE name IN
    ('operated_by', 'purchased_from', 'shipped_by', 'issued_by');

-- 4. Reconciliation: drop auto-created types no fact references (like 050).
DELETE FROM relationship_types
WHERE description LIKE 'Auto-created relationship_type: %'
  AND id NOT IN (SELECT DISTINCT relationship_type_id FROM facts);
