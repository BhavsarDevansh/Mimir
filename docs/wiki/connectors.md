# Connectors

> **Phase:** 3 — Connectors
> **Status:** Scaffolded (issue #178). Instance registry table + facade landed (issue #179). No connector syncs data yet.

## What connectors are

Connectors are how Mimir learns about your life from the services you use.
Each connector is a background worker that fetches data from a service — your
email, your calendar, your photo library — extracts facts from it, and stores
those facts in your knowledge graph alongside everything Mimir already knows.

Importantly, connectors are **not** a separate pipeline. They push data
through the exact same fact pipeline that conversations use, so connector-sourced
facts get the same confidence scoring, corroboration, supersession, and
sensitivity gating as facts you tell Mimir directly.

## What works right now

**No sync yet, but the registry is in place.** As of this version:

- The `mimir-connectors` crate exists and is wired into the workspace.
- The feature flags for the three core connector types (`photos`, `calendar`,
  `gmail`) are declared.
- The database-access boundary is in place: connectors talk to the knowledge
  graph only through its public facade and never touch the database pool
  directly.
- The `connectors` instance-registry table exists (issue #179 / F2). Each row
  is one configured connector instance (e.g. a single Gmail account) and
  stores its type, backend, config, lifecycle status (`Setup`/`Active`/
  `Paused`/`Error`), auth state (`Unauthenticated`/`Authenticated`/`Expired`),
  sync cursor, last sync time, and last error — so connectors survive daemon
  restarts. The `KnowledgeGraph` facade exposes `list_connectors`,
  `get_connector_by_slug`, `upsert_connector`, `update_sync_cursor`,
  `set_connector_status`, and `set_auth_state`.

There is no working backend yet — no calendar sync, no email fetch, no photo
watcher. The trait, registry, and mock connector are scaffolding stubs that
later issues will fill in.

## What is planned

- **Photos** — local photo library watching, EXIF/GPS extraction, place-fact
  derivation.
- **Calendar** — CalDAV read/sync, event → fact extraction, event-subsystem
  integration.
- **Email** — IMAP ingestion with IDLE, structured and LLM-based fact
  extraction (flights, bookings, contacts).

All data stays local-first. Secrets are stored per-connector with permission
validation; an OS keyring backend will be an opt-in extra.

## How to follow progress

See `VISION/09-Roadmap/Phase-3-Plan.md` for the full design and issue
breakdown, and `docs/connectors-framework.md` for the technical implementation
details.
