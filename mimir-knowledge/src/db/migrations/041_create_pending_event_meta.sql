-- 041: Pending event-shape metadata for sensitive facts (issue #74, PR #173).
--
-- Sensitive facts return `Pending` before the event-overlay block in extract.rs,
-- so the derived recurrence / event_type / auto_complete_policy /
-- requires_user_action would otherwise be lost across the confirmation
-- boundary and `confirm_fact` would rebuild a one-time `Reminder` overlay with
-- synthesised defaults. To rebuild the overlay faithfully on confirmation, the
-- event shape computed by `event_from_extraction` is persisted here, keyed on
-- the pending fact. The row is consumed (deleted) when the fact is confirmed
-- and the overlay is rebuilt; on rejection the fact is hard-deleted and the
-- `ON DELETE CASCADE` foreign key removes the metadata automatically.

CREATE TABLE pending_event_meta (
    fact_id INTEGER PRIMARY KEY REFERENCES facts(id) ON DELETE CASCADE,
    recurrence_type_id INTEGER NOT NULL REFERENCES recurrence_types(id),
    event_type_id INTEGER NOT NULL REFERENCES event_types(id),
    auto_complete_policy_id INTEGER NOT NULL REFERENCES auto_complete_policies(id),
    requires_user_action BOOLEAN NOT NULL
);
