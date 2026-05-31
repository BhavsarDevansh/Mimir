-- Rebuild fact_audit_log without FK CASCADE so audit rows survive fact deletion.

CREATE TABLE fact_audit_log_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id INTEGER NOT NULL,
    action TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT,
    performed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    performer TEXT
);

INSERT INTO fact_audit_log_new (id, fact_id, action, old_value, new_value, performed_at, performer)
SELECT id, fact_id, action, old_value, new_value, performed_at, performer
FROM fact_audit_log;

DROP TABLE fact_audit_log;

ALTER TABLE fact_audit_log_new RENAME TO fact_audit_log;

CREATE INDEX idx_fact_audit_log_fact ON fact_audit_log(fact_id);
CREATE INDEX idx_fact_audit_log_performed_at ON fact_audit_log(performed_at);
