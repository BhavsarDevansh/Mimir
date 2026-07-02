# Connectors Framework (mimir-connectors)

> **Phase:** 3 — Connectors
> **Status:** Scaffolded (issue #178 / F1). Trait, registry, and mock are stubs; filled by F6/F7/F13.
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

## What is NOT done in F1

- No `Connector` behaviour (auth, health, sync, extract, lifecycle).
- No `connectors` DB table, model, queries, or `KnowledgeGraph` facade
  additions (F2).
- No `sources` provenance FK migration (F3).
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
