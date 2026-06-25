-- 039: Events & reminders subsystem (issue #74).
--
-- Events are a lifecycle + recurrence overlay on facts. A fact with a future
-- `valid_from` is a one-time event; a fact tagged with recurrence (e.g. a
-- birthday) is a recurring event. The `events.upcoming_scan` job derives
-- overlays and applies deterministic auto-complete policies. Source facts
-- surface in the "Upcoming" memory section directly; the overlay only manages
-- lifecycle (status) and recurrence advancement.
--
-- `entity_dates` is superseded by this table and removed in the same change
-- set (no existing data to migrate).

CREATE TABLE event_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO event_types (id, name) VALUES
    (1, 'birthday'),
    (2, 'appointment'),
    (3, 'deadline'),
    (4, 'task'),
    (5, 'reminder'),
    (6, 'custom');

CREATE TABLE event_statuses (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO event_statuses (id, name) VALUES
    (1, 'Pending'),
    (2, 'Active'),
    (3, 'Completed'),
    (4, 'Dismissed'),
    (5, 'Snoozed');

CREATE TABLE auto_complete_policies (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO auto_complete_policies (id, name) VALUES
    (1, 'AutoCompleteOnDate'),
    (2, 'RequiresUserAction'),
    (3, 'RecurringYearly');

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL UNIQUE REFERENCES facts(id) ON DELETE CASCADE,
    entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    trigger_date TIMESTAMP NOT NULL,
    recurrence_type_id INTEGER NOT NULL DEFAULT 1 REFERENCES recurrence_types(id),
    event_type_id INTEGER NOT NULL DEFAULT 6 REFERENCES event_types(id),
    status_id INTEGER NOT NULL DEFAULT 2 REFERENCES event_statuses(id),
    auto_complete_policy_id INTEGER NOT NULL DEFAULT 1 REFERENCES auto_complete_policies(id),
    requires_user_action BOOLEAN NOT NULL DEFAULT FALSE,
    addressed_at TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_events_entity ON events(entity_id);
CREATE INDEX idx_events_trigger_date ON events(trigger_date);
CREATE INDEX idx_events_status ON events(status_id);
CREATE INDEX idx_events_recurrence ON events(recurrence_type_id);
