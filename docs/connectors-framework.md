# Connectors Framework (mimir-connectors)

> **Phase:** 3 — Connectors
> **Status:** Scaffolded (issue #178 / F1). Instance registry table + facade landed (issue #179 / F2). `sources` provenance FK migration landed (issue #180 / F3). Shared `normalize_and_insert` boundary landed (issue #181 / F4). Trait, registry, and mock are stubs; filled by F6/F7/F13.
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

`mimir-connectors` is the service ingestion framework for Mimir. Connectors
are background sync workers that fetch data from external services (email,
calendar, photos, …), normalize it, and insert it into the knowledge graph
through the *existing* fact pipeline — the same `normalize_and_insert`
boundary used by conversational `remember` calls. They are not a parallel
track.

## Database-access boundary

Connectors never hold a `sqlx` pool handle. All persistence goes through the
[`mimir_knowledge::KnowledgeGraph`] facade. Accordingly, `mimir-connectors`
depends on `mimir-core` and `mimir-knowledge` **only** and does **not** declare
a direct `sqlx` dependency (it enters the build graph only transitively, via
`mimir-knowledge`'s internal use).

## Shared ingestion boundary (F4 / #181)

The resolve → confidence → sensitivity-gate → insert orchestration lives in
`mimir-knowledge::normalize` as a single reusable function, so connector
ingestion and conversational `remember` extraction share one deterministic
Rust pipeline:

```rust
pub async fn normalize_and_insert(
    kg: &KnowledgeGraph,
    facts: Vec<NormalizedFact>,
    provenance: Provenance,
) -> Result<ExtractionOutcome, KnowledgeError>
```

- **`Provenance`** (batch-level) carries the connector instance id + type and
  the `extraction_method`. A connector sync calls this once per batch with a
  `Provenance::connector(instance_id, connector_type, method)`.
- **`NormalizedFact`** (per-fact) carries typed content (entity types, parsed
  temporal bounds, typed recurrence, validated category ids, sensitivity flag,
  optional correction scope) and the per-fact `raw_reference` (the native item
  id). `source_type` is per-fact (`Connector` for connector facts).
- **Confidence** is `confidence::initial(source_type, connector_type)` with no
  extraction-method discount. Corroboration / supersession / inference are
  inherited from `insert_fact_in_tx`, so cross-connector corroboration (Gmail
  flight + Calendar event on overlapping dates) is an explicit acceptance
  criterion, not an accident.

Because `mimir-connectors` depends on `mimir-knowledge`, it reaches these types
directly; it never needs a parallel insert path.

## Crate layout

| Module | Role | Filled by |
|--------|------|-----------|
| `connector` | Runtime `Connector` trait (identity accessors only) | F6 — full trait + `ConnectorMode`/`SyncOptions`/`HealthStatus` |
| `registry` | `ConnectorRegistry` (construction + length) | F7 — registration, lookup, multi-backend factory dispatch |
| `mock` | `MockConnector` (no-op identity impl) | F13 — configurable in-memory test harness |

Provenance types that connectors reference (`ConnectorType`, `SourceType`)
live in `mimir-knowledge` and are re-used, not duplicated (DRY).

## Feature flags

```toml
[features]
default = ["photos", "calendar", "gmail"]
photos = []   # Google/Apple/local photo ingestion (C1–C2)
calendar = [] # CalDAV calendar ingestion (C3–C4)
gmail = []    # IMAP email ingestion (C5–C7)
```

The framework core and the mock connector are **always built**. Running
`cargo build -p mimir-connectors --no-default-features` therefore still
compiles a working framework + mock harness — the gated backends are simply
absent. The feature flags are declared in F1 but currently gate no code; the
gated dependencies (`kamadak-exif`, `icalendar`, `async-imap`, `mail-parser`,
`oauth2`, `notify`, `keyring`) and backend modules land with C1–C7 / F10.

## Workspace wiring

- `mimir-connectors` is a workspace `members` entry.
- `mimir-server` depends on `mimir-connectors`; the daemon will own a
  `ConnectorRegistry` and `ConnectorSupervisor` once A1 wires them into
  `AppState` (not yet done in F1).

## Safety

`#![deny(unsafe_code)]` is enforced at the crate root, consistent with the
workspace-wide no-`unsafe` guarantee.

## Connector instance registry (F2)

Issue #179 added the `connectors` instance-registry table (migration
`042_create_connectors.sql`) plus its Rust model, queries, and
`KnowledgeGraph` facade methods. Each row is a single configured connector
instance — one Gmail account, one CalDAV calendar — so backends can persist
sync cursor, auth state, and health across daemon restarts.

### Schema

```sql
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
```

Two lookup tables mirror the `event_statuses` pattern: `connector_statuses`
(`Setup=1`, `Active=2`, `Paused=3`, `Error=4`) and `connector_auth_states`
(`Unauthenticated=1`, `Authenticated=2`, `Expired=3`). The Rust enums
`ConnectorStatus` and `ConnectorAuthState` (`#[repr(i16)]`, `sqlx::Type`)
live in `mimir-knowledge/src/models/enums.rs`. This deliberately uses typed
integer enums rather than the `TEXT` columns proposed in the issue, to match
the rest of the knowledge-graph schema and the project's "smallest data type"
rule.

> The `sources.connector_instance_id` provenance FK and the
> `SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?` item-count
> query landed in **F3 (#180)**: see [Sources provenance FK (F3)](#sources-provenance-fk-f3).

### Facade methods

`KnowledgeGraph` exposes: `list_connectors`, `get_connector_by_slug`,
`get_connector` (by id), `upsert_connector`, `update_sync_cursor`,
`set_connector_status`, and `set_auth_state`.

- `upsert_connector` is keyed on `slug`. `slug` and `connector_type` are
  immutable identity: on conflict it overwrites the mutable config surface
  (`backend`, `display_name`, `config_json`, `status`, `auth_state`) and bumps
  `updated_at`; it **preserves** `id`, `created_at`, and the sync-progress
  fields (`sync_cursor`, `last_sync_at`, `last_error`), which are owned by
  their dedicated mutators. Reusing an existing `slug` with a different
  `ConnectorType` returns `KnowledgeError::ConnectorTypeMismatch` rather than
  silently rewriting the instance's kind (which would leave the previous
  backend's type-specific sync state attached to a different connector type).
  The check is atomic: the `ON CONFLICT DO UPDATE ... WHERE
  connectors.connector_type_id = excluded.connector_type_id` guard updates zero
  rows on a mismatch, so `RETURNING` is empty and a clean error is surfaced.
- `set_connector_status` takes an `Option<Option<String>>` `error` argument:
  `None` leaves `last_error` untouched, `Some(None)` clears it to NULL, and
  `Some(Some(msg))` records `msg` (e.g. a circuit-breaker reason).
- Unknown ids return `KnowledgeError::ConnectorNotFound`; a slug reused with a
  different type returns `KnowledgeError::ConnectorTypeMismatch`. The
  `connector_type` field is the typed `ConnectorType` enum (variants map to
  seeded `connector_types` rows), so the `connector_types(id)` foreign key is
  the DB-level guard and no separate type-existence validation is needed.

## Sources provenance FK (F3)

Issue #180 migrated `sources.connector_id TEXT` to
`connector_instance_id INTEGER REFERENCES connectors(id)` (migration
`043_sources_connector_instance_fk.sql`). SQLite cannot change a column type in
place, so this is the standard table-rebuild dance; it is lossless for existing
DBs because legacy `sources.connector_id` values were already limited to `NULL`
or `''` (the insert paths differ — `queries/source.rs` normalised a missing
connector to `''`, `queries/fact.rs` bound `NULL`), and
both map to `connector_instance_id IS NULL`. `connector_type_id` is retained
(denormalised) so the confidence model can read the connector kind without a
join even when `connector_instance_id` is `NULL`.

The rebuild restores the NULL-aware unique index as
`(fact_id, source_type_id, COALESCE(connector_instance_id, 0), COALESCE(raw_reference, ''))`
— `0` is a safe sentinel because autoincrement ids start at `1` — plus a new
`idx_sources_instance` index for the item-count query.

### Rust model + validation gate

`Source.connector_instance_id: Option<i32>` and `NewFact.connector_instance_id:
Option<i32>` replace the old `Option<String>` labels; every insert site
(`extract.rs`, `queries/fact.rs` corroboration + `insert_fact_in_tx`,
`queries/trash.rs` restore, `optimization/mod.rs` dedup-merge) was updated.

The `insert_fact` confidence gate now keys connector provenance off
`connector_instance_id` rather than `connector_type`. When set it requires
`raw_reference` and `extraction_method`, resolves the instance, and **enforces
consistency**: if `connector_type` is also supplied it must match the instance's
registered `connector_type_id`, otherwise `KnowledgeError::Validation` is
returned; when `connector_type` is omitted it is **derived** from the instance.
An unregistered instance id is rejected (`ConnectorNotFound`-style validation).
The `forget --source <slug>` filter now matches `connectors.slug` via subquery
(plus the existing `source_types.name` arm), since the column is no longer a
free-form string.


## What is NOT done in F1

- No `Connector` behaviour (auth, health, sync, extract, lifecycle).
- No `connectors` DB table, model, queries, or `KnowledgeGraph` facade
  additions — **done in F2 (#179)**; see [Connector instance registry](#connector-instance-registry-f2).
- No `sources` provenance FK migration — **done in F3 (#180)**; see [Sources provenance FK (F3)](#sources-provenance-fk-f3).
- No `NormalizedFact`/`normalize_and_insert` refactor (F4).
- No entity-resolution enhancement (F5).
- No supervisor, secret store, rate limiter, or any backend.
- `mimir-server` does not yet use the crate beyond declaring the dependency.

## Verification

```bash
cargo build --workspace                              # full workspace
cargo build -p mimir-connectors --no-default-features # framework + mock only
cargo test -p mimir-connectors                        # scaffolding smoke test
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```
