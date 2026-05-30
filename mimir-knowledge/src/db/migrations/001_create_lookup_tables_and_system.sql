-- Lookup tables with stable integer IDs and Rust enum mappings.
-- All enums use #[repr(i16)] discriminants matching these seed values.

CREATE TABLE entity_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO entity_types (id, name) VALUES
    (1, 'Person'),
    (2, 'Place'),
    (3, 'Event'),
    (4, 'Object'),
    (5, 'Concept'),
    (6, 'Organization'),
    (7, 'Activity');

CREATE TABLE entity_date_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO entity_date_types (id, name) VALUES
    (1, 'Birth'),
    (2, 'Death'),
    (3, 'Anniversary'),
    (4, 'Created'),
    (5, 'Dissolved'),
    (6, 'Custom');

CREATE TABLE recurrence_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO recurrence_types (id, name) VALUES
    (1, 'None'),
    (2, 'Daily'),
    (3, 'Weekly'),
    (4, 'Monthly'),
    (5, 'Yearly');

CREATE TABLE location_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO location_types (id, name) VALUES
    (1, 'Home'),
    (2, 'Work'),
    (3, 'Visited'),
    (4, 'Origin'),
    (5, 'Current');

CREATE TABLE fact_statuses (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO fact_statuses (id, name) VALUES
    (1, 'Active'),
    (2, 'Inferred'),
    (3, 'Disputed'),
    (4, 'Corrected'),
    (5, 'Superseded'),
    (6, 'Forgotten');

CREATE TABLE relation_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO relation_types (id, name) VALUES
    (1, 'InferredFrom'),
    (2, 'Corrects'),
    (3, 'Supersedes');

CREATE TABLE source_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO source_types (id, name) VALUES
    (1, 'Email'),
    (2, 'Calendar'),
    (3, 'Photo'),
    (4, 'Message'),
    (5, 'Inference'),
    (6, 'UserEdit'),
    (7, 'Connector');

CREATE TABLE preference_categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO preference_categories (id, name) VALUES
    (1, 'NotificationStyle'),
    (2, 'CalendarAutoAdd'),
    (3, 'ProactivityLevel'),
    (4, 'CommunicationTone'),
    (5, 'Privacy');

CREATE TABLE preference_source_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO preference_source_types (id, name) VALUES
    (1, 'Explicit'),
    (2, 'Inferred'),
    (3, 'Corrected');

CREATE TABLE system_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
