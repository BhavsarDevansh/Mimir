# Connectors

> **Phase:** 3 — Connectors
> **Status:** Scaffolded (issue #178). Instance registry table + facade landed (issue #179). `sources` provenance FK landed (issue #180). Shared `normalize_and_insert` ingestion boundary landed (issue #181). Full entity-resolution chain landed (issue #182). The runtime `Connector` trait + data types landed (issue #183 / F6). **The `ConnectorRegistry` + multi-backend factory dispatch landed (issue #184 / F7).** No connector syncs data yet — backends arrive in later Phase 3 issues.

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
watcher. The `Connector` **trait and its data types are now defined**
(issue #183 / F6): every connector implements an async `Connector` interface
with `sync` (fetch raw items) → `extract` (produce `NormalizedFact`s), plus
`authenticate`, `health`, optional `act` write-back, and `forget`.

**The multi-backend registry is in place (issue #184 / F7).** The
`ConnectorRegistry` maps each `(connector_type, backend)` pair — e.g.
`(Email, imap)` or `(Calendar, caldav)` — to a `ConnectorFactory` that
constructs the right implementation from a connector's stored config. A
connector *type* is the reliability/provenance axis; a *backend* is the
provider implementation chosen per instance. Adding a new backend is a new
factory registration — no database change — and many backends can coexist
under one type. Reliability stays per-type, so a Gmail-IMAP fact and a future
Gmail-Graph fact share the same confidence scoring. The configurable mock
harness remains a stub (F13).

**The shared ingestion boundary is in place (issue #181 / F4).** Connectors
will build `NormalizedFact`s from their items and call
`mimir_knowledge::normalize::normalize_and_insert` with a connector `Provenance`,
funnelling through the exact same entity-resolution → confidence →
sensitivity-gate → insert pipeline as conversational `remember` calls. That
means connector facts get corroboration for free: a Gmail flight fact and a
Calendar event describing the same trip corroborate the single knowledge-graph
fact (a source is added, confidence is boosted) instead of creating a
duplicate.

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
