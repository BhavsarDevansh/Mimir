-- 042: Connector instance registry (issue #179 / Phase 3 F2).
--
-- Each row is a single configured connector instance (e.g. one Gmail account,
-- one CalDAV calendar). Connector backends (Photos/Calendar/Gmail) register
-- themselves here so sync cursor, auth state, and health persist across daemon
-- restarts. The `sources` provenance FK (`connector_instance_id`) is added in a
-- later migration (F3); item counts are therefore derivable only after F3.

CREATE TABLE connector_statuses (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO connector_statuses (id, name) VALUES
    (1, 'Setup'),
    (2, 'Active'),
    (3, 'Paused'),
    (4, 'Error');

CREATE TABLE connector_auth_states (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO connector_auth_states (id, name) VALUES
    (1, 'Unauthenticated'),
    (2, 'Authenticated'),
    (3, 'Expired');

CREATE TABLE connectors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    connector_type_id INTEGER NOT NULL REFERENCES connector_types(id),
    slug TEXT NOT NULL UNIQUE,
    backend TEXT NOT NULL,
    display_name TEXT NOT NULL,
    config_json TEXT NOT NULL,
    status_id INTEGER NOT NULL DEFAULT 1 REFERENCES connector_statuses(id),
    auth_state_id INTEGER NOT NULL DEFAULT 1 REFERENCES connector_auth_states(id),
    sync_cursor TEXT,
    last_sync_at TIMESTAMP,
    last_error TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_connectors_type ON connectors(connector_type_id);
CREATE INDEX idx_connectors_status ON connectors(status_id);
