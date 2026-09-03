-- 060: Closed taxonomy contracts + unrecognized-fact staging (#468)

ALTER TABLE relationship_types ADD COLUMN parent_id INTEGER REFERENCES relationship_types(id);
ALTER TABLE relationship_types ADD COLUMN depth INTEGER NOT NULL DEFAULT 1;
ALTER TABLE relationship_types ADD COLUMN node_kind TEXT NOT NULL DEFAULT 'leaf';
ALTER TABLE relationship_types ADD COLUMN emit_eligible INTEGER NOT NULL DEFAULT FALSE;
ALTER TABLE relationship_types ADD COLUMN definition TEXT NOT NULL DEFAULT '';
ALTER TABLE relationship_types ADD COLUMN render_template TEXT;
ALTER TABLE relationship_types ADD COLUMN dedup_key TEXT;
ALTER TABLE relationship_types ADD COLUMN temporal_policy TEXT NOT NULL DEFAULT 'none';
ALTER TABLE relationship_types ADD COLUMN sensitivity_policy TEXT NOT NULL DEFAULT 'inherit';
ALTER TABLE connectors ADD COLUMN facts_staged INTEGER NOT NULL DEFAULT 0;

CREATE TABLE relationship_type_category_rules (
    relationship_type_id INTEGER NOT NULL REFERENCES relationship_types(id) ON DELETE CASCADE,
    subject_entity_type_id INTEGER NOT NULL DEFAULT 0,
    object_entity_type_id INTEGER NOT NULL DEFAULT 0,
    event_type_id INTEGER NOT NULL DEFAULT 0,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    PRIMARY KEY (relationship_type_id, subject_entity_type_id, object_entity_type_id, event_type_id)
);

CREATE TABLE unrecognized_facts (
    id INTEGER PRIMARY KEY,
    connector_instance_id INTEGER REFERENCES connectors(id) ON DELETE CASCADE,
    raw_reference TEXT,
    relationship_type_raw TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'unmapped' CHECK (status IN ('unmapped', 'mapped', 'rejected')),
    proposed_relationship_type_id INTEGER REFERENCES relationship_types(id) ON DELETE SET NULL,
    resolution_note TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_relationship_types_parent ON relationship_types(parent_id);
CREATE INDEX idx_relationship_types_emit ON relationship_types(emit_eligible);
CREATE INDEX idx_unrecognized_facts_status ON unrecognized_facts(status);
-- COALESCE keeps chat-sourced rows (both source fields NULL) deduplicated as
-- well; a plain unique index treats NULLs as distinct in SQLite.
CREATE UNIQUE INDEX idx_unrecognized_facts_source ON unrecognized_facts(
    COALESCE(connector_instance_id, -1),
    COALESCE(raw_reference, ''),
    relationship_type_raw,
    payload_json
);

-- 1. Seed the compact upper ontology. Roots are query-only; leaves keep the
-- existing canonical vocabulary and gain a deterministic category fallback.
INSERT INTO relationship_types (name, definition, node_kind, emit_eligible, depth)
VALUES
    ('identity', 'Identity and biography facts', 'root', FALSE, 0),
    ('relationship', 'Human and social relationships', 'root', FALSE, 0),
    ('preference', 'Stable likes, dislikes, and preferences', 'root', FALSE, 0),
    ('employment', 'Work, roles, skills, and compensation', 'root', FALSE, 0),
    ('education', 'Study, degrees, and educational status', 'root', FALSE, 0),
    ('residence', 'Where the subject lives', 'root', FALSE, 0),
    ('location', 'Physical location and visited places', 'root', FALSE, 0),
    ('ownership', 'Owned possessions, accounts, and policies', 'root', FALSE, 0),
    ('event', 'Events, appointments, and temporal activities', 'root', FALSE, 0),
    ('travel', 'Trips, transport, and hospitality', 'root', FALSE, 0),
    ('commerce', 'Purchases, orders, deliveries, and payments', 'root', FALSE, 0),
    ('health', 'Health conditions, medications, and care', 'root', FALSE, 0),
    ('credential', 'Credentials, identifiers, and sensitive records', 'root', FALSE, 0),
    ('communication', 'Communication and rejected actions', 'root', FALSE, 0),
    ('document', 'Documents and artefacts', 'root', FALSE, 0)
ON CONFLICT(name) DO UPDATE SET
    node_kind = excluded.node_kind,
    emit_eligible = excluded.emit_eligible,
    depth = excluded.depth;

-- The positive-preference leaf replaces the former open favourite_* family.
INSERT INTO relationship_types
    (name, definition, node_kind, emit_eligible, depth, parent_id)
VALUES
    (
        'prefers',
        'A stable positive preference for a person, place, activity, or thing',
        'leaf',
        TRUE,
        1,
        (SELECT id FROM relationship_types WHERE name = 'preference')
    )
ON CONFLICT(name) DO UPDATE SET
    node_kind = excluded.node_kind,
    emit_eligible = excluded.emit_eligible,
    depth = excluded.depth,
    parent_id = excluded.parent_id;

-- 2. Put every existing canonical predicate into exactly one subtree. This
-- preserves existing IDs and leaves the old DAG table intact for query paths.
UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'identity'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('preferred_name', 'has_name', 'born_on', 'died_on', 'created_on');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'relationship'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('has_partner', 'has_parent', 'has_sibling', 'has_child');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'preference'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('prefers', 'hobby', 'dislikes');

-- `prefers` is the canonical positive-preference leaf. Legacy positive
-- preference rows remain queryable for migration, but their aliases now point
-- at the controlled leaf so they do not compete in the LLM vocabulary.
UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'preference'),
    depth = 1,
    node_kind = 'alias',
    emit_eligible = FALSE
WHERE name IN ('has_preference', 'likes', 'favourite_food', 'favourite_colour');

INSERT INTO relationship_type_aliases (alias, relationship_type_id)
VALUES
    ('has_preference', (SELECT id FROM relationship_types WHERE name = 'prefers')),
    ('likes', (SELECT id FROM relationship_types WHERE name = 'prefers')),
    ('loves', (SELECT id FROM relationship_types WHERE name = 'prefers')),
    ('favourite_food', (SELECT id FROM relationship_types WHERE name = 'prefers')),
    ('favourite_colour', (SELECT id FROM relationship_types WHERE name = 'prefers'))
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

INSERT INTO relationship_type_aliases (alias, relationship_type_id)
VALUES ('prefers', (SELECT id FROM relationship_types WHERE name = 'prefers'))
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

-- Common legacy residence wording remains queryable and ingestible through
-- the controlled leaf without resurrecting a competing predicate.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
VALUES ('lives_in', (SELECT id FROM relationship_types WHERE name = 'resides_in'))
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

-- Legacy Obsidian and informal fact wording remains ingestible while emitting
-- only the controlled canonical leaves.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
VALUES
    ('married_to', (SELECT id FROM relationship_types WHERE name = 'has_partner')),
    ('birthday', (SELECT id FROM relationship_types WHERE name = 'born_on')),
    ('allergic_to', (SELECT id FROM relationship_types WHERE name = 'allergy'))
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'employment'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('works_as', 'works_at', 'job_title', 'income', 'salary', 'skill');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'education'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('studied_at', 'studied', 'completed_degree', 'educational_status');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'residence'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name = 'resides_in';

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'location'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('located_in', 'visited');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'ownership'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('owns', 'has_pets', 'bank_account', 'credit_card', 'insurance');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'event'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('has_appointment', 'has_event', 'attending', 'took_photo_at', 'took_photo');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'travel'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('has_flight', 'departs_from', 'arrives_at', 'operated_by', 'has_booking', 'has_ticket', 'issued_by');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'commerce'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('has_order', 'purchased_from', 'has_delivery', 'shipped_by', 'delivered_to');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'health'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('health_condition', 'allergy', 'medication', 'diagnosis');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'credential'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name IN ('password', 'ssn', 'social_security_number');

UPDATE relationship_types
SET parent_id = (SELECT id FROM relationship_types WHERE name = 'communication'), depth = 1, node_kind = 'leaf', emit_eligible = TRUE
WHERE name = 'rejected_action';

-- 3. Existing abstract roots stay query-only.
UPDATE relationship_types
SET node_kind = 'root', emit_eligible = FALSE, parent_id = NULL, depth = 0
WHERE name IN ('residence', 'employment', 'education', 'containment');

-- 4. Deterministic category fallbacks for the controlled leaves. Entity-type
-- and event-type refinements can be added to the rule table by governance.
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 100 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'identity');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 400 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'relationship');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 700 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'preference');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 500 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'employment');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 550 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'education');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 610 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'residence');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 800 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'location');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 650 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'ownership');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 900 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'event');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 800 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'travel');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 680 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'commerce');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 300 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'health');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 170 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'credential');
INSERT OR IGNORE INTO relationship_type_category_rules (relationship_type_id, category_id)
SELECT id, 160 FROM relationship_types
WHERE parent_id = (SELECT id FROM relationship_types WHERE name = 'communication');

-- 5. Roots keep self-aliases so taxonomy queries and admin surfaces can
-- address them without relying on numeric IDs; they remain non-emitting.
INSERT INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types
WHERE node_kind = 'root'
ON CONFLICT(alias) DO UPDATE SET relationship_type_id = excluded.relationship_type_id;

-- Legacy auto-created rows remain queryable as aliases but are never part of
-- the controlled emit vocabulary; governance must promote them explicitly.
UPDATE relationship_types
SET node_kind = 'alias', emit_eligible = FALSE
WHERE description LIKE 'Auto-created relationship_type: %';
