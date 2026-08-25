-- LLM semantic entity dedup (issue #282): persist the LLM evaluation on
-- entity_merge_queue rows, mirroring the dedup_queue columns added by
-- migration 030. NULL means the row was flagged deterministically (alias
-- overlap) and not yet LLM-evaluated.
ALTER TABLE entity_merge_queue ADD COLUMN suggested_action TEXT;
ALTER TABLE entity_merge_queue ADD COLUMN llm_confidence REAL;
