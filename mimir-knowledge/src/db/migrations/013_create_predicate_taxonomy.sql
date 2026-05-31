-- Predicate taxonomy: named predicates with subject/object type constraints.

CREATE TABLE predicates (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT
);

CREATE TABLE predicate_constraints (
    predicate_id INTEGER NOT NULL REFERENCES predicates(id) ON DELETE CASCADE,
    allowed_subject_type_id INTEGER NOT NULL REFERENCES entity_types(id),
    allowed_object_type_id INTEGER NOT NULL REFERENCES entity_types(id),
    PRIMARY KEY (predicate_id, allowed_subject_type_id, allowed_object_type_id)
);

-- Seed common predicates
INSERT INTO predicates (id, name, description) VALUES
    (1, 'is_in', 'Subject is located inside or part of object'),
    (2, 'visited', 'Subject visited object'),
    (3, 'owns', 'Subject owns object'),
    (4, 'works_as', 'Subject works in the role of object'),
    (5, 'has_partner', 'Subject has a partnership relationship with object'),
    (6, 'has_parent', 'Subject has object as a parent'),
    (7, 'born_on', 'Subject was born on date object'),
    (8, 'died_on', 'Subject died on date object'),
    (9, 'located_in', 'Subject is physically located in object'),
    (10, 'created_on', 'Subject was created on date object');

-- Seed constraints (permissive: allow common combos; strict enforcement in app code)
-- is_in: Person->Place, Organization->Place, Object->Place, Place->Place
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (1, 1, 2), (1, 6, 2), (1, 4, 2), (1, 2, 2);

-- visited: Person->Place, Person->Event
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (2, 1, 2), (2, 1, 3);

-- owns: Person->Object, Organization->Object, Person->Place
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (3, 1, 4), (3, 6, 4), (3, 1, 2);

-- works_as: Person->Activity, Person->Organization
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (4, 1, 7), (4, 1, 6);

-- has_partner: Person->Person, Organization->Organization
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (5, 1, 1), (5, 6, 6);

-- has_parent: Person->Person
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (6, 1, 1);

-- born_on: Person->DateTime, Organization->DateTime
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (7, 1, 8), (7, 6, 8);

-- died_on: Person->DateTime, Organization->DateTime
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (8, 1, 8), (8, 6, 8);

-- located_in: Person->Place, Organization->Place, Object->Place, Event->Place
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (9, 1, 2), (9, 6, 2), (9, 4, 2), (9, 3, 2);

-- created_on: Person->DateTime, Organization->DateTime, Object->DateTime, Event->DateTime, Concept->DateTime
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (10, 1, 8), (10, 6, 8), (10, 4, 8), (10, 3, 8), (10, 5, 8);
