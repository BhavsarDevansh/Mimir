# Connectors

> **Phase:** 3 — Connectors
> **Status:** In progress (issue #178). Instance registry table + facade landed (issue #179). `sources` provenance FK landed (issue #180). Shared `normalize_and_insert` ingestion boundary landed (issue #181). Full entity-resolution chain landed (issue #182). The runtime `Connector` trait + data types landed (issue #183 / F6). The `ConnectorRegistry` + multi-backend factory dispatch landed (issue #184 / F7). The `ConnectorSupervisor` supervised lifecycle landed (issue #185 / F8). Manual sync triggering landed (issue #186 / F9). Connector secret store landed (issue #187 / F10). Shared rate-limit + retry/backoff primitives landed (issue #189 / F12). Configurable, always-compiled mock connector test harness landed (issue #190 / F13). **Two concrete backends have landed. The local-filesystem Photos connector (issue #195 / C1, enriched in C2 / #196) — a `notify` file watcher with EXIF GPS/datetime extraction and a per-file mtime/inode incremental cursor — and the CalDAV Calendar connector (issue #197 / C3) — a `Polling` CalDAV client (PROPFIND + sync-collection REPORT, sync-token incremental sync) with app-password and OAuth-refresh auth via the secret store, parsing VEVENTs with `icalendar`. The calendar connector is transport-only: it fetches and stages events; event → knowledge-graph fact extraction + reminders integration + write-back are C4 (#198).** Email backends arrive in later Phase 3 issues.

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

**The Photos and Calendar connectors sync data; email is not yet implemented.** As of this version:

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
  `touch_last_sync`, `set_connector_status`, and `set_auth_state`.

The first real backend is in: the **Photos** local-filesystem connector
(issue #195 / C1) — a read-only `notify` file watcher that extracts EXIF GPS +
datetime and emits a `took_photo` fact per photo (see [Photos Connector](photos-connector.md)).
The **Calendar** connector (see [Calendar Connector](calendar-connector.md)) speaks CalDAV and syncs events with an incremental sync-token cursor; its current `extract()` is transport-only and emits no `NormalizedFact`s yet (C4 / #198 does event → fact extraction). The `Connector` **trait and its data types are defined**
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
harness is now real (issue #190 / F13): see [Mock Connector](mock-connector.md).

**The supervised lifecycle is in place (issue #185 / F8).** A
`ConnectorSupervisor` owns one background task per connector whose lifecycle
status is `Active`, and centralises everything needed to keep a connector
running safely: spawn on startup, exponential backoff and restart on a failed
sync or a task panic, a circuit breaker that moves a connector to `Error`
after repeated consecutive failures (so it does not hot-loop), pausing when the
service reports expired auth, graceful shutdown over a shared `watch` channel,
and persistence of each connector's sync cursor, so a restart resumes from where
the last completed sync left off. `Paused`, `Error`, and `Setup` connectors are
not auto-started. The supervisor is a library component; daemon and CLI wiring
arrive in later Phase 3 issues (so `mimir stop` does not yet drive the
supervisor).

**Manual sync triggering is in place (issue #186 / F9).** The supervisor can be
asked to sync a connector immediately, instead of waiting for its next polling
interval. A `trigger_sync` call delivers options to the connector's runner —
`--full` forces a complete re-fetch (ignoring the saved cursor) and `--since`
limits the window — and waits for that cycle to finish, returning how many
items it fetched. Concurrent triggers on the same connector are serialised (they
queue and run one at a time, never in parallel), and triggering a connector that
is paused or errored reports that it is not running. This is the library
building block for the future `mimir connector sync <slug> [--full|--since]`
command, which lands once the daemon and CLI are wired in later Phase 3 issues.
Push-style connectors (like a future IMAP IDLE feed) do not have a polling
interval to preempt, so manual triggers are not supported for them yet.

**The shared ingestion boundary is in place (issue #181 / F4).** Connectors
will build `NormalizedFact`s from their items and call
`mimir_knowledge::normalize::normalize_and_insert` with a connector `Provenance`,
funnelling through the exact same entity-resolution → confidence →
sensitivity-gate → insert pipeline as conversational `remember` calls. That
means connector facts get corroboration for free: a Gmail flight fact and a
Calendar event describing the same trip corroborate the single knowledge-graph
fact (a source is added, confidence is boosted) instead of creating a
duplicate.

**The connector secret store is in place (issue #187 / F10).** A single
`SecretStore` handles every auth kind (OAuth 2.0, API token, app password),
persisted as one `0600` JSON file per connector under
`~/.local/share/mimir/secrets/`. Loads fail closed if permissions are too
loose, writes are atomic, and slugs are validated against path traversal. See
[How connector credentials are stored](#how-connector-credentials-are-stored)
below.

**Shared rate limiting + retry primitives are available (issue #189 / F12).** A
 per-instance rate limiter is ready for connectors to adopt: a token bucket for
 sustained requests-per-second and burst, an optional rolling 24h daily quota
 (which pauses the connector for the rest of the day instead of hanging), and
 automatic 429/502/503/504 retry with backoff + jitter honouring a server
 `Retry-After`. Connector LLM calls are exempt — they use the shared LLM worker
 pool. Connectors will wire these primitives into their outbound calls as their
 backends are implemented in later Phase 3 issues. See
 [Connector Rate Limiting & Retry](connector-rate-limiting.md).


**The configurable mock connector test harness is in place (issue #190 / F13).**
An always-compiled in-memory connector whose behaviour is driven entirely by its
`config_json`: it emits canned `NormalizedFact`s on a configurable cadence in
both polling and push modes, and can inject health/auth states, failures, and
panics to exercise the supervisor. It is the T1 sync→extract→insert→query
vehicle — the real `ConnectorSupervisor` + `KnowledgeGraph` ingest a mock's
canned facts end-to-end with correct connector provenance, without any real
service. See [Mock Connector](mock-connector.md).
## How connector credentials are stored

When you add a connector that needs a login (Gmail over OAuth, Fastmail over
an app password, Home Assistant over an API token), Mimir stores its
credentials in **one JSON file per connector**, under
`~/.local/share/mimir/secrets/<connector-slug>.json`. The file is readable
only by you (`0600`, group/other bits stripped) and the `secrets/` directory
is `0700`. Mimir refuses to *read* a secret whose file or directory has been
loosened (e.g. made world-readable) — it tells you to re-tighten the
permissions rather than risk leaking the credential.

Three kinds of credential are supported, all in the same store:

- **OAuth 2.0** — access token + optional refresh token + optional expiry
  (Gmail, Google Calendar).
- **API token** — a single bearer token (Home Assistant, GitHub PAT).
- **App password** — a single password string (Fastmail, legacy IMAP).

Credentials are stored **in plaintext**, deliberately — the same way your LLM
API key is stored in `config.toml` — because Mimir is a local-first app that
relies on your home directory being private (the home-directory trust
boundary). At-rest encryption is planned for a later release, and an
optional OS keyring backend (macOS Keychain / Linux Secret Service / Windows
Credential Manager) is tracked as a follow-up (#188) for those who prefer it.

Removing a connector (`mimir connector remove`) wipes its secret file. (The
`remove` CLI/server flow itself lands in a later Phase 3 issue; the secret
store already supports deletion.)


## What is planned

- **Photos** — local photo library watching + EXIF/GPS extraction + GPS → place reverse-geocoding landed (C1 / #195 + C2 / #196).
- **Calendar** — CalDAV transport (PROPFIND + sync-token sync + icalendar parse + app-password/OAuth-refresh auth) landed (C3 / #197); remaining work is event → KB fact extraction, events-subsystem (#74) integration, and write-back (C4 / #198).
- **Email** — IMAP ingestion with IDLE, structured and LLM-based fact
  extraction (flights, bookings, contacts).

All data stays local-first. Secrets are stored per-connector with permission
validation; an OS keyring backend will be an opt-in extra.

## How to follow progress

See `VISION/09-Roadmap/Phase-3-Plan.md` for the full design and issue
breakdown, and `docs/connectors-framework.md` for the technical implementation
details.
