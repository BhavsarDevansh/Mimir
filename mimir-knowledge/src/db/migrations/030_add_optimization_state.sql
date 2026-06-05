CREATE TABLE IF NOT EXISTS optimization_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at TIMESTAMP NOT NULL,
    finished_at TIMESTAMP,
    status TEXT NOT NULL,
    trigger TEXT NOT NULL,
    error TEXT
);

CREATE TABLE IF NOT EXISTS optimization_pass_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id INTEGER NOT NULL REFERENCES optimization_runs(id) ON DELETE CASCADE,
    pass_name TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TIMESTAMP NOT NULL,
    finished_at TIMESTAMP,
    facts_merged INTEGER NOT NULL DEFAULT 0,
    dedup_candidates_queued INTEGER NOT NULL DEFAULT 0,
    facts_forgotten INTEGER NOT NULL DEFAULT 0,
    error TEXT
);

ALTER TABLE dedup_queue ADD COLUMN suggested_action TEXT;
ALTER TABLE dedup_queue ADD COLUMN llm_confidence REAL;
