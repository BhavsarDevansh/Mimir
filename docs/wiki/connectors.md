# Connectors

> **Phase:** 3 — Connectors
> **Status:** In progress (issue #178). Instance registry table + facade landed (issue #179). `sources` provenance FK landed (issue #180). Shared `normalize_and_insert` ingestion boundary landed (issue #181). Full entity-resolution chain landed (issue #182). The runtime `Connector` trait + data types landed (issue #183 / F6). The `ConnectorRegistry` + multi-backend factory dispatch landed (issue #184 / F7). The `ConnectorSupervisor` supervised lifecycle landed (issue #185 / F8). Manual sync triggering landed (issue #186 / F9). Connector secret store landed (issue #187 / F10). Shared rate-limit + retry/backoff primitives landed (issue #189 / F12). Configurable, always-compiled mock connector test harness landed (issue #190 / F13). The daemon owns the connector framework at startup and exposes connector CRUD/status HTTP routes (A1 / #202). Connector action routes — sync, pause/resume, OAuth token ingest, write-back dispatch, and the forget cascade — landed (A2 / #203). The `mimir connector …` CLI landed (A3 / #204), and the interactive OAuth PKCE loopback flow landed (A4 / #205): `mimir connector add` / `auth` with an `auth.kind=oauth` config runs the flow in the CLI process — ephemeral loopback listener, browser-opened authorize URL (printed first for headless sessions), code exchange, token POST to the daemon's ingest route. OAuth token refresh now runs on the vetted `oauth2` crate (5.0.0, `default-features = false`) through a custom HTTP adapter over the workspace's single reqwest 0.13 client — no duplicate HTTP/TLS stack (issue #240). **Three concrete backends have landed and all three emit knowledge-graph facts.** The local-filesystem Photos connector (issue #195 / C1, enriched in C2 / #196) — a `notify` file watcher with EXIF GPS/datetime extraction and a per-file mtime/inode incremental cursor — emits a `took_photo_at <place>` fact per photo when GPS resolves a place name, a `visited <coords-label>` fact when GPS has no place name (issue #250), and a `took_photo <rel_path>` record when there is no GPS. The CalDAV Calendar connector (issue #197 / C3, enriched in C4 / #198) — a `Polling` CalDAV client (PROPFIND + sync-collection REPORT, sync-token incremental sync) with app-password and OAuth-refresh auth via the secret store — extracts event facts (`user has_event`, `located_in`, `attending`) and supports write-back (`create_event` / `update_event` / `delete_event`). The **IMAP Email connector** (issue #199 / C5, enriched in C6 / #200, C7 / #201, and #249) — an `async-imap` client (IMAP `LOGIN` / `AUTHENTICATE XOAUTH2`, `UID FETCH` incremental sync, `IDLE` push with a polling fallback, a UIDVALIDITY-safe last-UID cursor, and a hand-rolled TCP+rustls TLS handshake) — extracts normalized facts from mail, including iMIP invites, `schema.org` JSON-LD reservations, and LLM-extracted flights/bookings from prose. The T1 integration/E2E harness landed (issue #206): daemon-level tests drive the real CLI against an in-process daemon with the mock connector's `facts` knob, verifying the full sync → `normalize_and_insert` → KB-query round trip — `source_type=Connector`, instance provenance, reliability-score confidence, and the corroboration path.

## What connectors are

Connectors are how Mimir learns about your life from the services you use. Each connector is a background worker that fetches data from a service — your email, your calendar, your photo library — extracts facts from it, and stores those facts in your knowledge graph alongside everything Mimir already knows.

Importantly, connectors are **not** a separate pipeline. They push data through the exact same fact pipeline that conversations use, so connector-sourced facts get the same confidence scoring, corroboration, supersession, and sensitivity gating as facts you tell Mimir directly.

## What works right now

**The Photos, Calendar, and Email (IMAP) connectors sync data and emit facts.** As of this version:

- The `mimir-connectors` crate exists and is wired into the workspace.
- The feature flags for the three core connector types (`photos`, `calendar`, `gmail`) are declared.
- The database-access boundary is in place: connectors talk to the knowledge graph only through its public facade and never touch the database pool directly.
- The `connectors` instance-registry table exists (issue #179 / F2). Each row is one configured connector instance (e.g. a single Gmail account) and stores its type, backend, config, lifecycle status (`Setup`/`Active`/ `Paused`/`Error`), auth state (`Unauthenticated`/`Authenticated`/`Expired`), sync cursor, last sync time, and last error — so connectors survive daemon restarts. The `KnowledgeGraph` facade exposes `list_connectors`, `get_connector_by_slug`, `upsert_connector`, `update_sync_cursor`, `update_durable_state`, `update_sync_progress_and_durable_state`, `touch_last_sync`, `set_connector_status`, and `set_auth_state`.

The first real backend is in: the **Photos** local-filesystem connector (issue #195 / C1) — a read-only `notify` file watcher that extracts EXIF GPS + datetime and emits one fact per photo (`took_photo_at <place>`, `visited <coords-label>`, or `took_photo <rel_path>`, depending on what the GPS resolves; see [Photos Connector](photos-connector.md)). The **Calendar** connector (see [Calendar Connector](calendar-connector.md)) speaks CalDAV and syncs events with an incremental sync-token cursor; its `extract()` turns VEVENTs into the appointment fact cluster (`user has_event <event>` typed `EventType::Appointment`, `<event> located_in <place>`, `<attendee> attending <event>`) and its `act()` write-back creates/updates/deletes remote events (C4 / #198). The **Email** connector (see [Email Connector](email-connector.md)) speaks IMAP — `LOGIN` / `AUTHENTICATE XOAUTH2`, `IDLE` push with a polling fallback, and `UID FETCH` incremental sync with a UIDVALIDITY-safe cursor; its `extract()` runs a deterministic cascade (iMIP invites via C6 / #200, `schema.org` JSON-LD reservations via #249) with a last-resort LLM layer for unstructured prose such as flight and booking confirmations (C7 / #201). The `Connector` **trait and its data types are defined** (issue #183 / F6): every connector implements an async `Connector` interface with `sync` (fetch raw items) → `extract` (produce `NormalizedFact`s), plus `authenticate`, `health`, optional `act` write-back, and `forget`.

**The multi-backend registry is in place (issue #184 / F7).** The `ConnectorRegistry` maps each `(connector_type, backend)` pair — e.g. `(Email, imap)` or `(Calendar, caldav)` — to a `ConnectorFactory` that constructs the right implementation from a connector's stored config. A connector *type* is the reliability/provenance axis; a *backend* is the provider implementation chosen per instance. Adding a new backend is a new factory registration — no database change — and many backends can coexist under one type. Reliability stays per-type, so a Gmail-IMAP fact and a future Gmail-Graph fact share the same confidence scoring. The configurable mock harness is now real (issue #190 / F13): see [Mock Connector](mock-connector.md).

**The supervised lifecycle is in place (issue #185 / F8).** A `ConnectorSupervisor` owns one background task per connector whose lifecycle status is `Active`, and centralises everything needed to keep a connector running safely: spawn on startup, exponential backoff and restart on a failed sync or a task panic, a circuit breaker that moves a connector to `Error` after repeated consecutive failures (so it does not hot-loop), pausing when the service reports expired auth, graceful shutdown over a shared `watch` channel, and persistence of each connector's sync cursor, so a restart resumes from where the last completed sync left off. `Paused`, `Error`, and `Setup` connectors are not auto-started. The daemon owns the supervisor at startup (A1 / #202): it restores `Active` runners, drains them on graceful shutdown, and exposes the action routes (A2 / #203) — `POST /connectors/{id}/pause` and `/resume` — so `mimir stop` does drive the supervisor. `ConnectorSupervisor::start(id)` (re-spawn one connector) is the internal supervisor method behind `resume`, not an HTTP route. The `mimir connector …` CLI plumbing is A3 (#204).

**Manual sync triggering is in place (issue #186 / F9).** The supervisor can be asked to sync a connector immediately, instead of waiting for its next polling interval. A `trigger_sync` call delivers options to the connector's runner — `--full` forces a complete re-fetch (ignoring the saved cursor) and `--since` limits the window — and waits for that cycle to finish, returning how many items it fetched. Concurrent triggers on the same connector are serialised (they queue and run one at a time, never in parallel), and triggering a connector that is paused or errored reports that it is not running. This is the library building block behind `POST /connectors/{id}/sync` (A2 / #203) and the future `mimir connector sync <slug> [--full|--since]` CLI command (A3 / #204). Push-style connectors (like a future IMAP IDLE feed) do not have a polling interval to preempt, so manual triggers are not supported for them yet.

**The shared ingestion boundary is in place (issue #181 / F4).** Connectors will build `NormalizedFact`s from their items and call `mimir_knowledge::normalize::normalize_and_insert` with a connector `Provenance`, funnelling through the exact same entity-resolution → confidence → sensitivity-gate → insert pipeline as conversational `remember` calls. That means connector facts get corroboration for free: a Gmail flight fact and a Calendar event describing the same trip corroborate the single knowledge-graph fact (a source is added, confidence is boosted) instead of creating a duplicate.

**The connector secret store is in place (issue #187 / F10).** A single `SecretStore` handles every auth kind (OAuth 2.0, API token, app password), persisted as one `0600` JSON file per connector under `~/.local/share/mimir/secrets/`. Loads fail closed if permissions are too loose, writes are atomic, and slugs are validated against path traversal. See [How connector credentials are stored](#how-connector-credentials-are-stored) below.

**Shared rate limiting + retry primitives are available (issue #189 / F12).** A per-instance rate limiter is ready for connectors to adopt: a token bucket for sustained requests-per-second and burst, an optional rolling 24h daily quota (which pauses the connector for the rest of the day instead of hanging), and automatic 429/502/503/504 retry with backoff + jitter honouring a server `Retry-After`. Connector LLM calls are exempt — they use the shared LLM worker pool. Connectors will wire these primitives into their outbound calls as their backends are implemented in later Phase 3 issues. See [Connector Rate Limiting & Retry](connector-rate-limiting.md).


**The daemon exposes connector management and actions over HTTP (A1 / #202, A2 / #203).** The daemon owns the `ConnectorRegistry` and `ConnectorSupervisor` at startup, restores `Active` runners, and exposes a REST surface: `GET /connectors` (list with derived item counts), `POST /connectors` (register an instance in `Setup`), `GET/DELETE /connectors/{id}` (show; `DELETE` detaches provenance so ingested facts survive), and the action routes — `POST /connectors/{id}/sync` (trigger a manual sync), `POST /connectors/{id}/pause` and `/resume` (lifecycle control), `POST /connectors/{id}/tokens` (ingest an OAuth token / API token / app password and flip the connector to `authenticated`), `POST /connectors/{id}/actions` (dispatch a write-back like creating a calendar event), and `POST /connectors/{id}/forget` (cascade-forget: trash every fact the connector sourced, delete its stored credential, and remove the row). `forget` is recoverable from trash for 30 days, unlike `DELETE` which keeps the facts. The `mimir connector …` CLI that drives these routes is A3 (#204), and the interactive OAuth PKCE login that obtains the first token is A4 (#205) — both implemented.

**The configurable mock connector test harness is in place (issue #190 / F13).** An always-compiled in-memory connector whose behaviour is driven entirely by its `config_json`: it emits canned `NormalizedFact`s on a configurable cadence in both polling and push modes, and can inject health/auth states, failures, and panics to exercise the supervisor. It is the T1 sync→extract→insert→query vehicle — the real `ConnectorSupervisor` + `KnowledgeGraph` ingest a mock's canned facts end-to-end with correct connector provenance, without any real service. See [Mock Connector](mock-connector.md).

## How connector credentials are stored

When you add a connector that needs a login (Gmail over OAuth, Fastmail over an app password, Home Assistant over an API token), Mimir stores its credentials in **one JSON file per connector**, under `~/.local/share/mimir/secrets/<connector-slug>.json`. The file is readable only by you (`0600`, group/other bits stripped) and the `secrets/` directory is `0700`. Mimir refuses to *read* a secret whose file or directory has been loosened (e.g. made world-readable) — it tells you to re-tighten the permissions rather than risk leaking the credential.

Three kinds of credential are supported, all in the same store:

- **OAuth 2.0** — access token + optional refresh token + optional expiry (Gmail, Google Calendar).
- **API token** — a single bearer token (Home Assistant, GitHub PAT).
- **App password** — a single password string (Fastmail, legacy IMAP).

Credentials are stored **in plaintext**, deliberately — the same way your LLM API key is stored in `config.toml` — because Mimir is a local-first app that relies on your home directory being private (the home-directory trust boundary). At-rest encryption is planned for a later release, and an optional OS keyring backend (macOS Keychain / Linux Secret Service / Windows Credential Manager) is tracked as a follow-up (#188) for those who prefer it.

Removing a connector wipes its secret file: `DELETE /connectors/{id}` deletes the slug-keyed secret before the row, and `POST /connectors/{id}/forget` deletes it as part of the cascade. The `mimir connector remove` CLI subcommand that plumbs these routes lands in A3 (#204).


## What is planned

- **Photos** — local photo library watching + EXIF/GPS extraction + GPS → place reverse-geocoding landed (C1 / #195 + C2 / #196).
- **Calendar** — CalDAV transport, event → KB fact extraction, events-subsystem (#74) integration, write-back, and server-side deletion → KB fact lifecycle all landed (C3 / #197 + C4 / #198 + #247).
- **Email** — IMAP transport (C5 / #199), structured extraction (C6 / #200), `schema.org` JSON-LD extraction (#249), LLM extraction for flights/bookings (C7 / #201), and the interactive OAuth PKCE login (A4 / #205) all landed.

All data stays local-first. Secrets are stored per-connector with permission validation; an OS keyring backend will be an opt-in extra.

## How to follow progress

See `VISION/09-Roadmap/Phase-3-Plan.md` for the full design and issue breakdown, and `docs/connectors-framework.md` for the technical implementation details.

## Managing connectors (daemon routes)

The daemon owns the connector framework and exposes the connector management (A1 / #202) and action (A2 / #203) routes. The `ConnectorRegistry` and `ConnectorSupervisor` are constructed at startup: the built-in Photos (`local`), Calendar (`caldav`), and Email (`imap`) factories are registered behind their cargo features, and the supervisor is wired with the shared geocoder, the `FileSecretStore`, the configured user identity (so the Calendar connector authors `user has_event` and the Photos connector authors `took_photo_at` against the canonical user entity), and the shared `Arc<dyn LlmBackend>` (so the Email prose-extraction layer routes through the system queue). `Active` connector runners are restored at startup and drained on graceful shutdown.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/connectors` | List every registered instance with derived item counts and last-sync/health |
| `POST` | `/connectors` | Register a new instance (add-only) |
| `GET` | `/connectors/{id}` | Show a single instance with its derived item count |
| `DELETE` | `/connectors/{id}` | Stop the runner, detach provenance, delete the instance |
| `POST` | `/connectors/{id}/sync` | Trigger a manual sync (F9) |
| `POST` | `/connectors/{id}/pause` | Stop the runner and flip `Paused` |
| `POST` | `/connectors/{id}/resume` | Re-spawn the runner and flip `Active` |
| `POST` | `/connectors/{id}/tokens` | Ingest credentials and flip `auth_state` (loopback-only) |
| `POST` | `/connectors/{id}/actions` | Dispatch a write-back action to `act()` |
| `POST` | `/connectors/{id}/forget` | Cascade-forget facts, secret, and row (loopback-only) |

`POST /connectors` is **add-only**: it validates the `(connector_type, backend)` pair against the daemon's registry (rejecting an unregistered backend with `400`), rejects an existing `slug` with `409`, and creates the instance in `Setup` status (it is not started until `resume` moves it to `Active`). Slug uniqueness is enforced atomically by an insert that relies on the `connectors.slug UNIQUE` index, so two concurrent `POST /connectors` for the same slug cannot both succeed — one wins and the other gets `409 Conflict`. The request body carries `connector_type`, `backend`, `slug`, `display_name`, and `config_json` (a backend-specific JSON object).

`DELETE /connectors/{id}` stops the runner (via `ConnectorSupervisor::stop(id)`, a no-op when no runner exists), deletes the connector's slug-keyed secret-store entry so a later connector with the same slug cannot load the deleted instance's credentials, then deletes the row. The secret is removed before the row, so a credential-deletion failure leaves the instance intact (the request returns `500` and the row is not removed) rather than a deleted row with lingering credentials. The `sources.connector_instance_id` foreign key is nulled first, so the connector's already-ingested facts survive with degraded provenance.

Status responses carry a derived `item_count`: the number of `sources` rows attributed to the instance, computed on demand via `KnowledgeGraph::count_sources_for_connector`. The `connectors` table itself stores no count column.

`POST /connectors/{id}/forget` is the full cascade: it marks the instance `Paused` (so an aborted cascade leaves a state a retry can reason about), stops the runner and runs the connector's local `forget()` cleanup, deletes the slug-keyed secret, trashes every fact the connector sourced (recoverable from trash for 30 days), and deletes the row. The cascade is serialised per connector and loopback-only, and the secret is deleted before the irreversible fact trash so a credential-deletion failure aborts with nothing destroyed.

## Managing connectors from the CLI

The `mimir connector` command group (A3 / #204) plumbs these routes so you never need to call the daemon by hand. Every subcommand except `remove` supports `--json` for scriptable output; slug-based commands resolve slugs client-side against the instance list. See [CLI Commands](cli-commands.md) for the full reference with examples.

```bash
# Add (created in Setup — resume activates it)
mimir connector add gmail --backend imap host=imap.gmail.com auth.kind=app_password auth.username=me@gmail.com

# Activate and sync
mimir connector resume gmail
mimir connector sync gmail --since 7d

# Status and lifecycle
mimir connector status gmail
mimir connector pause gmail

# Teardown — remove and forget are alternatives, not a sequence (remove deletes
# the row, so a later forget on the same slug cannot resolve it)
mimir connector remove gmail --yes       # detaches provenance; facts survive
# or
mimir connector forget gmail --yes       # trashes the connector's facts (recoverable 30 days)

# Write-back (Calendar)
mimir connector act calendar create_event '{"summary":"Lunch","start":"2026-08-12T12:00:00Z"}'
```

Non-OAuth configs (`auth.kind=app_password`) prompt for the credential via `inquire` and ingest it through the daemon's token route; pass `--password`/`--token` to supply it non-interactively, or run `mimir connector auth <slug>` later to complete or refresh credentials on an existing instance. OAuth configs (`auth.kind=oauth`) run the interactive PKCE login (A4 / #205) instead of prompting: the CLI opens the provider's authorize URL in your browser (the URL is printed first, so headless/SSH sessions can open it manually), receives the redirect on an ephemeral loopback listener, exchanges the code, and POSTs the token bundle to the daemon — the credentials are stored and `auth_state` becomes `authenticated`, but the instance stays in `Setup` until you run `mimir connector resume <slug>` to start its runner. Re-authing an expired OAuth connector is the same flow: `mimir connector auth <slug> auth.kind=oauth auth.auth_uri=… auth.token_endpoint=… auth.client_id=…` (the daemon does not expose the stored config, so the OAuth fields are re-supplied); after re-authing, run `mimir connector resume <slug>` if the connector is not running.

## How OAuth authentication works

OAuth connectors (Gmail, Google Calendar) authenticate with an access token that expires — typically after an hour. Mimir stores the access token, refresh token, and expiry in the connector's secret file, and **refreshes automatically**: when a sync starts with an expired (or nearly expired) token, the connector POSTs the stored refresh token to the provider's token endpoint, stores the fresh token bundle, and continues the sync with it. The refresh runs on the vetted `oauth2` library (issue #240), talks HTTP over the same reqwest stack as the rest of Mimir, never follows redirects, and only talks to HTTPS endpoints (or your own machine's loopback). If a refresh fails — for example because the provider revoked the refresh token — the connector flips to `auth_state=expired`, pauses, and you re-authenticate with `mimir connector auth <slug> auth.kind=oauth …` (re-supplying the OAuth client config), which runs the interactive PKCE login again.
