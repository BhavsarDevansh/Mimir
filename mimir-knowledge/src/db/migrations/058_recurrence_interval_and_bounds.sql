-- 058: Recurrence interval and series bounds on event overlays (PR #513 review).
--
-- The events subsystem advanced recurring overlays by kind only (weekly,
-- monthly, ...), so a fortnightly event advanced weekly and a bounded series
-- advanced indefinitely. The connector extractors now carry the full RRULE
-- (retaining interval, day/month constraints, and COUNT/UNTIL verbatim) plus
-- the denormalized interval and effective series end the scan engine uses.

ALTER TABLE events ADD COLUMN recurrence_rule TEXT;
ALTER TABLE events ADD COLUMN recurrence_interval INTEGER NOT NULL DEFAULT 1;
ALTER TABLE events ADD COLUMN recurrence_until TIMESTAMP;

-- The pending-event shape persisted across the sensitivity gate must carry the
-- same recurrence fields, otherwise confirming a sensitive recurring fact
-- rebuilds the overlay with a kind-only recurrence and loses the interval and
-- series bounds.
ALTER TABLE pending_event_meta ADD COLUMN recurrence_rule TEXT;
ALTER TABLE pending_event_meta ADD COLUMN recurrence_interval INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pending_event_meta ADD COLUMN recurrence_until TIMESTAMP;
