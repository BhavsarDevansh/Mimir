-- Add pending_confirmation flag for sensitive facts awaiting user confirmation.
-- Hot cache in KnowledgeGraph; DB is always the source of truth.

ALTER TABLE facts ADD COLUMN pending_confirmation BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX idx_facts_pending ON facts(pending_confirmation)
WHERE pending_confirmation = TRUE;
