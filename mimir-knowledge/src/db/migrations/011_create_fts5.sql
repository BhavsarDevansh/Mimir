CREATE VIRTUAL TABLE entity_fts USING fts5(
    name,
    aliases,
    content='entities',
    content_rowid='id'
);

-- Triggers to keep FTS5 index in sync with entities table
CREATE TRIGGER entities_fts_insert AFTER INSERT ON entities BEGIN
    INSERT INTO entity_fts(rowid, name, aliases)
    VALUES (new.id, new.name, new.aliases);
END;

CREATE TRIGGER entities_fts_delete AFTER DELETE ON entities BEGIN
    INSERT INTO entity_fts(entity_fts, rowid, name, aliases)
    VALUES ('delete', old.id, old.name, old.aliases);
END;

CREATE TRIGGER entities_fts_update AFTER UPDATE ON entities BEGIN
    INSERT INTO entity_fts(entity_fts, rowid, name, aliases)
    VALUES ('delete', old.id, old.name, old.aliases);
    INSERT INTO entity_fts(rowid, name, aliases)
    VALUES (new.id, new.name, new.aliases);
END;
