CREATE TABLE trash (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    original_table TEXT NOT NULL,
    original_id INTEGER NOT NULL,
    payload TEXT NOT NULL, -- JSON of the deleted row
    deleted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP,
    restored_at TIMESTAMP,
    restorer TEXT
);

CREATE INDEX idx_trash_table ON trash(original_table);
CREATE INDEX idx_trash_deleted_at ON trash(deleted_at);
