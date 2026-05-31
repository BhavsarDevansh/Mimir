CREATE TABLE connector_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE connector_reliability (
    connector_type_id INTEGER PRIMARY KEY REFERENCES connector_types(id),
    score REAL NOT NULL CHECK (score >= 0.0 AND score <= 1.0)
);

INSERT INTO connector_types (id, name) VALUES
    (1, 'Gmail'),
    (2, 'Calendar'),
    (3, 'Photos'),
    (4, 'LinkedIn');

INSERT INTO connector_reliability (connector_type_id, score) VALUES
    (1, 0.85),
    (2, 0.90),
    (3, 0.80),
    (4, 0.75);

ALTER TABLE sources ADD COLUMN connector_type_id INTEGER REFERENCES connector_types(id);
