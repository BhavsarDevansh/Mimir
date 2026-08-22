# Phase 3: Connectors — Implementation Plan

> **Status:** Designed. All eight architectural decisions (A–H) locked.
>
> **Created:** 2026-07-01
>
> **Depends on:** Phase 1 (Core Agent), Phase 2 (Knowledge Graph)
>
> **GitHub:** https://github.com/BhavsarDevansh/Mimir
>
> **Scope:** Connector framework + one backend per type (Photos, Calendar, Email) + CLI/server + tests. Cloud photos, Microsoft Graph calendar, RSS, etc. are follow-on work enabled by the multi-backend factory.

---

## 1. Goal

Build the connector framework and implement one backend per core connector type, so Mimir can ingest data from the user's email, calendar, and photo libraries into the Knowledge Graph as connector-provenanced facts. Connectors are background sync workers that fetch → normalize → insert facts through the *existing* KB pipeline, not a parallel track.

## 2. Locked Architectural Decisions

### A. Crate structure
- New crate `mimir-connectors` (`→ mimir-core`, `→ mimir-knowledge`; **no `sqlx` dependency** — DB access only via `KnowledgeGraph` facade). `#![deny(unsafe_code)]` at root.
- `mimir-server → mimir-connectors` so the daemon owns a `ConnectorRegistry`.
- Runtime `Connector` trait + `ConnectorRegistry` live in `mimir-connectors`; provenance `ConnectorType` stays in `mimir-knowledge`.
- Feature flags: `default = ["photos","calendar","gmail"]`; framework + `mock` always built.
- `ConnectorRegistry` constructed in `start_server`, stored in `AppState`; each connector runs as a supervised task.

### B. Extraction reuse (DRY)
- Extract a shared `normalize_and_insert(kg, Vec<NormalizedFact>, provenance) -> ExtractionOutcome` boundary from `extract.rs`. Both the conversational `remember` path and connectors funnel through it.
- One `NormalizedFact` type exported from `mimir-knowledge`; structured-parse and LLM-extraction both produce it, differing only in `extraction_method`.
- Confidence = `confidence::initial(SourceType::Connector, connector_type)` (per-connector reliability score). **No extraction-method discount** (keep simple).
- **Same sensitivity gate** as conversational facts (2c-i): sensitive connector facts land as `pending_confirmation` and surface via `kb audit`. Sensitivity system overhaul deferred.
- Enhance `resolve_entity` to full exact → alias → FTS5 fuzzy → create chain (closes a Phase 2 gap; noisy connector data needs it).
- Corroboration/supersession/inference inherited from `insert_fact_in_tx` — cross-connector corroboration is an explicit acceptance criterion, not an accident.

### C. Sync state + DB-access boundary
- New `connectors` table: `id INTEGER PRIMARY KEY AUTOINCREMENT`, `connector_type_id` FK, `slug TEXT NOT NULL UNIQUE`, `backend TEXT NOT NULL`, `display_name`, `config_json`, `status_id` / `auth_state_id` FKs (typed integer enums, not TEXT — see below), `sync_cursor TEXT`, `last_sync_at`, `last_error`, timestamps. (No text IDs — integer PKs per project convention; slug is a label, not a key.) `status` and `auth_state` are stored as `status_id` / `auth_state_id` foreign keys into `connector_statuses` (`Setup`/`Active`/`Paused`/`Error`) and `connector_auth_states` (`Unauthenticated`/`Authenticated`/`Expired`) lookup tables, mirrored by `#[repr(i16)]` Rust enums — same pattern as `event_statuses`/`EventStatus`. Implemented in F2 (#179).
- `sources.connector_id TEXT` → `connector_instance_id INTEGER REFERENCES connectors(id)` (safe migration; the column currently holds a mix of `NULL` and `''` because the insert paths differ — `queries/source.rs` normalises missing to `''`, `queries/fact.rs` binds `NULL`; the rebuild must treat both as "no instance"). `connector_type_id` retained. `Source` Rust struct updated. (F3 / #180.)
- Migration + `models/connector.rs` + `queries/connector.rs` + `KnowledgeGraph` facade in `mimir-knowledge`. Connectors **never** hold `&SqlitePool`.
- Opaque per-connector sync cursor; item counts derived from `sources`; paused/health/auth persisted for restart survival.

### D. Orchestration
- Connectors run as supervised long-lived tasks under a `ConnectorSupervisor`; each declares `Polling { interval, jitter }` or `Push` mode via the trait.
- **Not** routed through `BackgroundScheduler`/`JobQueue`; status via `connectors` table columns.
- Restart + exponential backoff + circuit breaker (N consecutive failures → `status='error'`, stop auto-restart, require manual `resume`); `auth_state='expired'` → paused + flagged.
- Manual sync via per-connector `Notify` + serialisation semaphore.
- Shutdown: observe `shutdown_tx`, persist sync cursor before exit.
- Startup: spawn `active` connectors after core state (KG/LLM) is up.
- Yield-on-user-activity: **deferred** for V1.

### D′. Connector LLM calls route through the shared pool
- Connector LLM calls go through the shared `LlmWorkerPool` via `Arc<dyn LlmBackend>`, on the **system queue** (priority below user chat). Guarantees no API-concurrency violation for one-call-at-a-time providers.
- Connectors receive `Arc<dyn LlmBackend>` at construction. Extraction uses **small per-item LLM calls** to bound user-chat latency (the user queue preempts between calls).

### E. Auth & secret storage
- `SecretStore` trait in `mimir-connectors`; V1 = `FileSecretStore` (plaintext, `0600` file / `0700` dir, one file per connector, perm validation). `keyring` as **opt-in** feature-gated backend, default off.
- `SecretBundle` enum: `OAuth { access_token, refresh_token, expires_at } | ApiToken(String) | AppPassword(String)`.
- OAuth PKCE (`oauth2 5.0.0`): **CLI runs the ephemeral loopback callback server**, opens the auth URL, exchanges the code, POSTs the token bundle to a daemon route for storage. `oauth2` is pulled with `default-features = false` and talks HTTP through a custom `AsyncHttpClient` adapter over the workspace's single reqwest 0.13 client (`OAuthHttpClient`, issue #240) — its optional reqwest 0.12 dependency never enters the tree.
- Refresh-on-sync; refresh failure → `auth_state='expired'`, paused, manual re-auth.
- `connector remove` wipes secret + row; `forget` cascades (drops facts with that `connector_instance_id` via existing trash machinery).
- At-rest encryption (argon2 + chacha20poly1305) **deferred** — marginal gain over 0600 unless keychain-anchored; revisit with the sensitivity overhaul.

### F. Multi-backend architecture
- `connector_type` (Email/Calendar/Photos) = provenance + reliability axis (fixed, seeded). `backend` = provider implementation (IMAP, CalDAV, local-FS, …), selected per instance. Many backends per type, growing over time.
- `ConnectorRegistry` maps `(connector_type, backend) → ConnectorFactory`. `backend` stored as a column on `connectors`. New backends = register a new factory, no schema change.
- Reliability stays **per-type** (not per-backend) for now.
- Phase 3 ships **one backend per type**: local-Photos, CalDAV-Calendar, IMAP-Email. Cloud photos, Microsoft Graph, RSS, etc. are follow-on issues.

### G. Tool-vs-connector disambiguation
- Issues #83, #93–#105 are function-calling **tools**, not sync connectors → relabeled `connectors` → `tools`, out of Phase 3.
- `connectors` label reserved for `Connector`-trait sync workers.
- **#98 Geocoding folds into Phase 3** as a pluggable `Geocoder` (free OSM Nominatim default) serving Photos, entity-locations, and the #98 tool.
- #97 RSS = first post-Phase-3 connector candidate (simple, no OAuth).

### H. Pre-existing Phase 3 issues
- **#65 entity locations** → Phase 3 core sub-issue (pairs with Geocoder; blocks Photos).
- **#66 Obsidian watcher** → standalone, blocked-on #62/#120, reuses Phase 3 watcher infra.
- **#67 pattern consolidation** → standalone, parallel KG work.
- **#69 kb heatmap/reset** → standalone, deferred CLI polish — **delivered (v0.135.0)**: `mimir kb heatmap` (daemon `GET /kb/heatmap` aggregates + terminal bar charts + `--json`) and `mimir kb reset` (interactive exact-phrase confirmation, 5-second countdown, daemon-side backup + hard delete via the shared forget-all machinery).

## 3. Build order

Framework + Mock → Photos (local) → Calendar (CalDAV) → Email (IMAP). Ascending risk: no-auth → auth → auth+LLM+push.

## 4. Version-checked dependency ledger

| Crate | Version | Used by | Feature-gated |
|-------|---------|---------|---------------|
| `oauth2` | 5.0.0 (`default-features = false`) | Calendar, Email, CLI PKCE | `oauth` (enabled by `calendar`, `gmail`; also enabled by the `mimir` binary) |
| `webbrowser` | 1.2.4 | CLI PKCE (A4): opens the provider's authorize URL in the default browser (cross-platform, MIT/Apache-2.0) | `mimir` binary |
| `url` | 2.x (in tree) | CLI PKCE (A4): parses the loopback callback query string | `oauth` (mimir-connectors) |
| `async-imap` | 0.11.3 | Email (IMAP + IDLE) | `gmail` |
| `mail-parser` | 0.11.5 | Email parsing | `gmail` |
| `icalendar` | 0.17.6 | Calendar parse + build; Email iMIP VEVENTs (shared with `gmail`) | `calendar`, `gmail` |
| `kamadak-exif` | 0.6.1 | Photos EXIF/GPS | `photos` |
| `notify` | 8.2.0 | Photos file watcher (already in tree) | `photos` |
| `keyring` | 4.1.2 | Optional secret backend | `secrets-keyring` (off by default) |
| `argon2` / `chacha20poly1305` | — | Deferred (at-rest encryption) | not in V1 |

All HTTP via `reqwest` (already a workspace dep). No `sqlx` in `mimir-connectors`.

**Reconciliation decision (#240, v0.96.0):** `oauth2` 5.0.0's optional `reqwest` feature pins reqwest 0.12, which would duplicate the workspace's reqwest 0.13 HTTP/TLS stack. The chosen path is `oauth2` with `default-features = false` plus a custom [`OAuthHttpClient`](../../docs/oauth-client.md) adapter that implements the crate's `AsyncHttpClient` trait over the workspace reqwest 0.13 client — one reqwest major in the tree, the vetted PKCE/refresh protocol code, redirects disabled on OAuth calls, and the pre-existing HTTPS/loopback endpoint gate + secret-hygiene error mapping preserved. oauth2 5.0.0's unconditional deps are already in the tree except `rand 0.8` (a third rand line alongside 0.9/0.10; small and required by oauth2's PKCE verifier generation) and `thiserror 1.x` (already in the tree). No reqwest 0.12-compatible oauth2 release exists (latest is still 5.0.0 as of 2026-08-11), so "wait for an upgrade" is struck from the options.

**Reconciliation decision (#239, v0.102.1):** the ledger now reflects the MSRV-capped `icalendar` resolution. The workspace `rust-version` is 1.85, and every `icalendar` release from 0.17.7 onwards requires Rust 1.88, so Cargo resolves the `icalendar = "0.17"` declaration (in `mimir-connectors`, shared by the `calendar` and `gmail` features) to 0.17.6 — the latest 1.85-compatible release. The parser API used by the calendar connector (the nom-backed low-level `icalendar::parser` plus the builder) is fully present in 0.17.6, so the workspace toolchain stays at MSRV 1.85 and the ledger pins 0.17.6 (not 0.17.12); revisit only when a feature introduced in 0.17.7+ is actually needed. The `async-imap` 0.11.3 and `mail-parser` 0.11.5 rows were corrected to the declared versions in `mimir-connectors/Cargo.toml` while reconciling the ledger.

## 5. Issue breakdown

Issues are tagged `phase-3`. Dependency references use "Blocked by: #N". Full specs live in the issue bodies; this doc is the design source of truth.

### Epic 1 — Connector Framework
- **F1** Scaffold `mimir-connectors` crate + workspace + feature flags
- **F2** `connectors` table + model + queries + `KnowledgeGraph` facade
- **F3** `sources` provenance FK migration (`connector_instance_id`) + `Source` struct
- **F4** `NormalizedFact` + `normalize_and_insert` DRY refactor (+ remember refactor + cross-connector corroboration test)
- **F5** Full entity-resolution chain (alias + FTS5) in `resolve_entity`
- **F6** `Connector` trait + data types (`ConnectorMode`, `SyncOptions`, `HealthStatus`)
- **F7** `ConnectorRegistry` + multi-backend factory dispatch
- **F8** `ConnectorSupervisor` — supervised lifecycle (spawn/restart/backoff/circuit-breaker/startup-restore/graceful-shutdown/cursor-persistence)
- **F9** Manual sync triggering (per-connector `Notify` + serialisation)
- **F10** `SecretStore` trait + `SecretBundle` + `FileSecretStore`
- **F11** `keyring` opt-in `SecretStore` backend *(deferred)*
- **F12** Rate limiter + retry/backoff primitives
- **F13** Mock connector (test harness, always compiled)

### Epic 2 — KB Supporting Work
- **S1** Geocoder service (`Geocoder` trait + OSM Nominatim forward/reverse + rate limiting)
- **S2** Geocoding conversational tool (#98 wrapper) *(deferred)*
- **S3** Entity locations write path + temporal bounds + geocode wiring (#65)
- **S4** Entity locations proximity queries (`find_nearby`)

### Epic 3 — Connectors (one backend per type)
- **C1** Photos: file watcher + EXIF extraction + incremental sync cursor
- **C2** Photos: GPS → place fact extraction + `entity_locations` wiring
- **C3** Calendar: CalDAV client + read/sync (sync-token, PROPFIND/REPORT)
- **C4** Calendar: event → KB fact extraction + events-subsystem (#74) integration + write-back
- **C5** Email: IMAP client + IDLE + incremental UID sync + auth (XOAUTH2 / app-password)
- **C6** Email: mail parsing + structured fact extraction (headers/dates/contacts)
- **C7** Email: LLM extraction (flights/bookings) via shared `LlmWorkerPool` system queue

### Epic 4 — CLI + Server
- **A1** Server: `AppState` registry/supervisor wiring + connector CRUD/status routes
- **A2** Server: action routes (sync/pause/resume) + OAuth token-ingest + `forget` cascade
- **A3** CLI: `mimir connector` subcommands plumbing to daemon
- **A4** CLI: OAuth PKCE loopback callback flow — **implemented (v0.97.0)**: `mimir connector add` / `auth` with an `auth.kind=oauth` config run the flow in the CLI process (`mimir-connectors::oauth::pkce`): ephemeral loopback listener on `127.0.0.1:0`, browser-opened authorize URL (printed first for headless sessions), CSRF state validation, code exchange via the shared `OAuthHttpClient`, token POST to the daemon's ingest route. `CalendarAuthMethod::OAuth` / `EmailAuthMethod::OAuth` gained a required `auth_uri` field (breaking config change).

### Epic 5 — Testing
- **T1** Integration/E2E test harness (mock connector sync → normalize → insert → query)
- **T2** Mock OAuth server + PKCE flow + rate-limit/backoff + supervisor edge-case tests

## 6. Dependency graph

```
F1 ─┬─> F6 ─> F7 ─┬─> F8 ─> F9 ─────────────────────┐
F2 ─┤            └─> F13 ────────────────────────────┤
F3 ─┤                                             ┌──┤
F4 ─┼─> F5                                       │  │
F12 ─┘                                           │  │
F10 ─> F11(deferred)                             │  │
                                                 │  ├─> C1 ─> C2 ─┐
F12 ─> S1 ─> S2(deferred)                        │  │              │
F2,F4,S1 ─> S3 ─> S4                             │  ├─> C3 ─> C4 ─┤
                                                 │  │              ├─> A1 ─> A2 ─> A3 ─> A4 ─> T2
                                                 │  ├─> C5 ─> C6 ─> C7 ────────────────────> T1
                                                 │  │
F2,F7,F8,F10 ───────────────────────────────────┘  │
                                                   │
F4,F7 ─> F13 ──────────────────────────────────────┘
```

Critical path (first vertical slice): **F1 → F6 → F7 → F8 → C1 → C2** (Photos). OAuth connectors path: **F10 → C3 / C5 → A4** (the interactive PKCE flow landed in v0.97.0).

## 7. Out of scope (follow-on issues, enabled by this framework)
- Cloud photo backends (Apple Photos library, Nextcloud, Google Photos)
- Microsoft Graph calendar backend (Outlook/Office 365)
- RSS connector (#97) — first post-Phase-3 connector
- At-rest secret encryption (argon2 + chacha20poly1305)
- Sensitivity system overhaul
- #66 Obsidian bidirectional watcher (blocked on #62/#120)
- #67 pattern consolidation, #69 kb heatmap/reset
- Function-calling tools #83, #93–#105 (relabeled `tools`)
