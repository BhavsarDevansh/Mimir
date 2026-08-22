# Mimir

**A persistent personal intelligence that learns from your life, reasons across your data, and becomes more useful the longer you use it.**

Named after the Norse god Mimir — the keeper of wisdom whose severed head preserved all knowledge and gave counsel to the gods. Mimir remembers everything, connects disparate facts, and helps you navigate the labyrinth of your own life.

## What Is Mimir?

Mimir is not a chatbot. It is a stateful, ever-learning companion that:

- **Learns implicitly** — observes your patterns, extracts facts, and builds a persistent knowledge graph of your life
- **Reasons intelligently** — investigates complex questions across multiple data sources, showing its work in real time
- **Acts proactively** — earns your trust over time, then anticipates your needs before you ask
- **Stays private** — local-first architecture. Your data stays on your device. No cloud intermediary.

## Core Principles

1. **Persistence over ephemerality** — Every interaction, fact, and preference is stored, versioned, and retrievable
2. **Implicit learning** — The agent observes, generalizes, and adjusts without requiring explicit training
3. **User sovereignty** — You can inspect, edit, and delete anything it knows. The knowledge base is yours
4. **Thoroughness** — When answering, it investigates all available avenues rather than settling on the first plausible answer
5. **Proactivity** — As confidence in its model of you grows, it anticipates needs rather than only responding to prompts
6. **Openness** — OpenAI-compatible API endpoint for all LLM needs; pluggable connectors for services

## Architecture

Mimir is a **Rust workspace** with a modular, local-first design. Your data stays on your device — there is no cloud intermediary.

### Crates

- **`mimir`** — the single binary: the daemon plus the `mimir` CLI (`start`, `chat`, `ask`, `status`, `memory`, `personality`, `kb`, `connector`, `stop`).
- **`mimir-server`** — Axum HTTP server with SSE streaming, sessions, and graceful shutdown; exposes an OpenAI-compatible chat endpoint.
- **`mimir-core`** — LLM client, configuration, context, tools, skills, and the personality system.
- **`mimir-knowledge`** — the SQLite knowledge graph: entities, facts, temporal reasoning, live memory condensation, events/reminders, and proximity queries.
- **`mimir-connectors`** — the pluggable service-ingestion framework and its backends: the OSM Nominatim geocoder, the local-filesystem Photos connector (EXIF + file watcher), the CalDAV Calendar connector (sync-token incremental sync), and the IMAP Email connector (IDLE push + UID incremental sync + a three-layer extraction cascade: iMIP calendar invites #200, schema.org JSON-LD #249, and LLM extraction of unstructured prose #201) today.
- **`mimir-api-types` / `mimir-client`** — shared wire types and the daemon HTTP client used by the CLI.

### Key subsystems

- **Knowledge graph** — entities, versioned facts, temporal reasoning, and live memory condensation, organised by a category-first ontology (predicate aliases for verb canonicalisation + Dewey categories with subtree retrieval). Regenerated on a schedule by the unified background scheduler.
- **Learning** — a server-side hooks engine (`remember.chat`) extracts facts after each non-incognito turn, debounced per session and idle-gated; facts flow through a deterministic Rust pipeline (`normalize_and_insert`) that enforces confidence, overwrite, and sensitive-fact policy. An on-demand Librarian extraction API is also available.
- **Retrieval agent** — ephemeral research agents that investigate the knowledge graph and conversation history on behalf of the main agent before answering complex questions.
- **Events & reminders** — a lifecycle + recurrence overlay on facts that surfaces upcoming birthdays, appointments, deadlines, and tasks, with a deterministic scan job for auto-completion and recurring advancement.
- **Connectors (Phase 3, in progress)** — pluggable workers that sync external services into the graph as connector-provenanced facts. The framework is in place: an object-safe `Connector` trait with two-step ingestion, a multi-backend `ConnectorRegistry`, a supervised lifecycle (restart with backoff, circuit breaker, auth-expiry pausing, graceful shutdown), a type-filtered entity-resolution chain (exact name → alias → FTS5 fuzzy → create new), and a pluggable `Geocoder` trait with an OSM Nominatim default backend. The first concrete backend — a read-only local-filesystem Photos connector (`notify` file watcher + `kamadak-exif` GPS/datetime extraction + a per-file mtime/inode incremental cursor) — is in. Photos are stored as facts (`took_photo_at <place>`), not entities: EXIF GPS is reverse-geocoded to a locality-level place name via the shared geocoder, photos at the same place corroborate into one open-ended fact, and the place's coordinates are anchored so proximity queries resolve places by where they are. When GPS resolves no place name, the photo's real-world event still becomes a fact (`visited <coords-label>`) with the file path kept as `raw_reference` provenance — never as the fact's object (issue #250). The knowledge graph grows with distinct places visited, not photo count. The second backend — a CalDAV Calendar connector (#197, #198) — is in: it speaks CalDAV (PROPFIND + sync-collection REPORT) over the shared HTTP client with sync-token incremental sync and `icalendar` VEVENT parsing, authenticating via an app password or a refreshed OAuth token loaded from the secret store. C4 (#198) adds event → knowledge-graph extraction: `extract()` drains VEVENTs into a cluster of facts — `user has_event <event>` (typed `Appointment`, recurrence from `RRULE`), `<event> located_in <place>`, `<attendee> attending <event>` — resolved via the full F5 entity chain, with future-dated/recurring events surfacing in the “Upcoming” section, and the only connector write-back, `act()` creating/updating/deleting remote events via CalDAV `PUT`/`DELETE`. The third backend — an IMAP Email connector (#199) — is in: it speaks IMAP over a hand-rolled TCP+rustls handshake (`LOGIN` / `AUTHENTICATE XOAUTH2`, `IDLE` push with a polling fallback auto-detected via CAPABILITY, `UID FETCH` incremental sync with a UIDVALIDITY-safe cursor, `BODY.PEEK[]` so mail stays unread), authenticating via an app password or a refreshed OAuth token. C6 (#200) adds deterministic structured extraction: `extract()` runs an extraction cascade over the staged RFC 822 messages and, today, turns iMIP calendar invites (`text/calendar` MIME parts whose `METHOD` — resolved from the MIME `method` parameter or the iCalendar body — is `REQUEST` or `REPLY`; if both sources are present and disagree the part is rejected) into the same appointment fact cluster the Calendar connector emits (`user has_event <event>` typed `Appointment`, plus `located_in` and `attending`), reusing a shared `ical` module (DRY). Invitation facts are keyed by the VEVENT `UID` (the stable iMIP identity), and a `CANCEL` trashes the cancelled event's facts through the shared tombstone machinery (issue #283), so a cancelled meeting stops surfacing in “Upcoming”; transactional facts keep the email's UIDVALIDITY-qualified IMAP UID as provenance. No per-email communication facts and no `Person` entities auto-created from `From`/`To` headers, so a message without a supported `REQUEST`/`REPLY` part produces no junk. Deterministic `schema.org` JSON-LD extraction for transactional email (#249) is in: `extract()` scans `text/html` parts for `<script type="application/ld+json">` blocks and emits typed fact clusters for `Order`, `ParcelDelivery`, `FlightReservation`, `LodgingReservation`, `EventReservation`, `Ticket`, and `ReservationPackage` — `user has_flight <flight>` / `has_booking <hotel>` / `has_order <order>` / `has_delivery <tracking>` / `has_ticket <ticket>` with appropriate `EventType` hints, plus secondary facts for airports, airlines, venues, carriers, and merchants — no LLM, pure Rust parsing. C7 (#201) adds the third, last-resort layer — LLM extraction of unstructured prose (free-text flights, bookings, bank statements, job offers) that deterministic layers cannot read: messages the deterministic layers produced no facts for are sent to the injected `LlmBackend` under a strict `extract_email_facts` tool schema on the shared system queue, and every field the LLM returns is validated in Rust against the typed enums before it becomes a fact, so an unrecognised value is dropped, never trusted. As of v0.92.0 the daemon owns the connector framework at startup (A1 / #202) and exposes the full connector action surface (A2 / #203): it constructs and registers the built-in Photos/Calendar/Email factories, wires the supervisor with the shared geocoder/secret-store/user-identity/LLM backend, restores `Active` runners on startup, and exposes connector CRUD/status HTTP routes (`GET/POST /connectors`, `GET/DELETE /connectors/{id}`) plus action routes — `POST /connectors/{id}/sync` (manual sync), `/pause` / `/resume` (lifecycle), `/tokens` (OAuth/app-password/token ingest + auth-state flip), `/actions` (write-back dispatch, e.g. Calendar event create/update/delete), and `/forget` (cascade-forget a connector's sourced facts, secret, and row) — with derived item counts. As of v0.95.0 the `mimir connector …` CLI (A3 / #204) plumbs all of it: `add` (with `key=value` config pairs and non-OAuth credential prompts), `auth` (complete or refresh credentials on an existing instance), `list`, `status`, `sync`, `pause`, `resume`, `remove`, `forget`, and `act` (write-back), each with `--json` output except `remove`; as of v0.96.0 OAuth token refresh runs on the vetted `oauth2` crate (5.0.0, `default-features = false`) through a custom HTTP adapter over the workspace's single reqwest 0.13 client — no duplicate HTTP/TLS stack, redirects disabled, secret-safe errors (issue #240). As of v0.97.0 the interactive OAuth PKCE loopback flow (A4 / #205) completes the CLI: `mimir connector add` / `auth` with an `auth.kind=oauth` config runs the flow in the CLI process — it binds an ephemeral loopback listener, opens the provider's authorize URL in the browser (printed first for headless/SSH sessions), validates the CSRF state, exchanges the code via the shared `OAuthHttpClient`, and POSTs the token bundle to the daemon's ingest route, so the instance becomes `authenticated` with the daemon never running a transient HTTP server. As of v0.98.0 the T1 integration/E2E harness (issue #206) proves the whole chain: daemon-level tests drive the real CLI against an in-process daemon with the mock connector's `facts` knob and verify the full sync → `normalize_and_insert` → KB-query round trip — `source_type=Connector`, instance provenance, reliability-score confidence, and the corroboration path (a second instance boosts confidence; a re-sync is a no-op). As of v0.108.0 connector discovery (issue #271) is in: `GET /connectors/catalog` exposes every registered `(connector_type, backend)` pair (feature-gated registrations included, sorted deterministically), `mimir connector catalog` renders it as a table or `--json`, and `mimir connector add` pre-flights the requested pair against the catalog before prompting for credentials, so a typo'd backend fails immediately with the supported set instead of after an interactive flow.
- **Entity locations** — a "where" fact becomes a typed `entity_locations` row with geocoding of the missing half (address → coords or coords → place) and temporal bounds that model moves (home 2020–2023, home 2023–present); re-stating the same place with an overlapping period confirms the existing row instead of duplicating it. Proximity queries (`find_nearby`) use a SQLite bounding-box pre-filter backed by a composite coordinate index, then an exact Haversine post-filter in pure Rust, with optional temporal scoping.

## Installation

> Coming soon. For now clone and run with `cargo run`.

## Quick Start

```bash
# Start the daemon
mimir start

# Ask a one-shot question
mimir ask "What is the capital of France?"

# Chat interactively with conversation history
mimir chat

# Check daemon status and configuration
mimir status

# View the live condensed memory block
mimir memory

# List available personality presets
mimir personality list

# Force memory condensation immediately
mimir memory --refresh

# Query the knowledge graph audit log
mimir kb audit --entity "Alice" --change-type status_change

# List sensitive facts awaiting confirmation
mimir kb pending

# Confirm or reject a pending sensitive fact
mimir kb confirm 42
mimir kb reject 42 --reason "entered in error"

# Visualise knowledge density or wipe the graph (with confirmation)
mimir kb heatmap
mimir kb reset

# Manage connectors (email, calendar, photos)
mimir connector list
mimir connector add gmail --backend imap host=imap.gmail.com auth.kind=app_password auth.username=me@gmail.com
mimir connector resume gmail
mimir connector status gmail
mimir connector sync gmail --since 7d

# Stop the daemon gracefully
mimir stop
```

## Configuration

Mimir auto-initialises its config directory on first run. The main config file lives at:

```
~/.config/mimir/config.toml
```

You can override settings with environment variables (e.g. `MIMIR_BASE_URL`).

Run `mimir init` for a guided first-run setup including identity configuration and optional systemd user service installation.

> **Note:** The legacy `memory.md` file-backed memory system was removed in v0.37.0. Memory is now served live from the knowledge graph.

## Documentation

The full project vision, architecture, and design documentation lives in the `VISION/` directory:

- `00-Overview/` — Vision statement, user values, success criteria
- `01-Core-Agent/` — CLI/chat UX, personality system, skills framework
- `02-Knowledge-Graph/` — Data model, temporal facts, learning modes, audit
- `03-Connectors/` — Connector framework, supported services, auth patterns
- `04-Reasoning-Engine/` — Investigation model, meta-threads, real-time streaming
- `05-Proactive-Agent/` — Trust ladder, pattern recognition, attention management
- `06-Vision-Tracking/` — Object detection, spatial memory
- `07-Journeys/` — End-to-end user scenarios and examples
- `08-Architecture/` — Security, privacy, deployment, integration points
- `09-Roadmap/` — Phased implementation plans

Technical implementation docs (per-subsystem, including OAuth, testing, and the connector framework) live in [docs/](docs/), and user-facing feature docs in [docs/wiki/](docs/wiki/).

## License

[GNU General Public License v3.0](LICENSE)

Mimir is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

## Contributing

Contributions are welcome! See `CONTRIBUTING.md` (coming soon) for guidelines.

## Acknowledgments

- Named after **Mimir**, the Norse keeper of wisdom whose severed head preserved all knowledge
- Built with Rust, SQLite, and an OpenAI-compatible LLM of your choice
