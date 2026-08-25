-- Entity semantic dedup pass counter (issue #282), mirroring
-- dedup_candidates_queued for the fact-level pass.
ALTER TABLE optimization_pass_runs ADD COLUMN entity_merges_queued INTEGER NOT NULL DEFAULT 0;
