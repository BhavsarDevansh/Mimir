# Mimir — Implementation Context

> **Created:** 2025-05-20
> **Last Updated:** 2026-08-06
> **Vision Docs:** `VISION/` directory (48 files, 10 sections)
> **Phase 1 Plan:** `VISION/09-Roadmap/Phase-1-Core-Agent.md`
> **GitHub:** https://github.com/BhavsarDevansh/Mimir

---

## What Is Mimir?

Mimir is a persistent, personal intelligence that learns from your life, connects to your services, and becomes more useful the longer you use it. It is NOT a chatbot — it is a stateful, ever-learning companion.

**Name origin:** Mimir, the Norse god of wisdom whose severed head preserved all knowledge and gave counsel to the gods.

**License:** GNU GPL-3.0

---

## Core Principles

1. **Persistence over ephemerality** — Every interaction, fact, and preference is stored, versioned, and retrievable
2. **Implicit learning** — The agent observes, generalizes, and adjusts without explicit training
3. **User sovereignty** — Inspect, edit, and delete anything. The knowledge base is yours
4. **Thoroughness** — Investigates all available avenues, not just the first plausible answer
5. **Proactivity** — Earns trust, then anticipates needs rather than only responding
6. **Openness** — OpenAI-compatible API endpoint; pluggable connectors for services
7. **Local-first** — All data stays on your device. No cloud intermediary

---

## Architecture Overview

### Single Binary, Two Modes

Mimir is distributed as a single `mimir` binary that operates in two modes:

```
mimir (single binary)
├── Daemon mode (mimir start)
│   ├── Axum HTTP server (bind_addr + socket_path)
│   ├── LlmWorkerPool (shared across all requests)
│   ├── ContextManager (shared across all sessions)
│   ├── ToolRegistry + SkillRegistry
│   ├── MemoryManager + MemoryLoader
│   └── Future: connectors, proactive agent, reasoning engine
└── Client mode (mimir ask, chat, status, memory, stop)
    └── mimir-client (HTTP client → daemon)
```

### Library Crates (code organisation, not separate binaries)

| Crate | Type | Role |
|-------|------|------|
| `mimir-core` | library | LLM client, config, memory, context, personality, tools, skills, paths |
| `mimir-server` | library | Axum routes, state, middleware |
| `mimir-client` | library | HTTP client for talking to the daemon |
| `mimir-knowledge` | library | SQLite knowledge graph (entities, facts, temporal queries, provenance) |
| `mimir-api-types` | library | Shared serde wire types decoupling server and client |
| `mimir-connectors` | library | Service ingestion framework — connectors fetch external data and normalize it into KB facts; DB access only via the `KnowledgeGraph` facade (Phase 3) |
| `mimir` | binary | Single entry point — dispatches daemon or client mode |

### Transport

The daemon exposes its API over two transports simultaneously:
1. **Unix domain socket** (`~/.local/share/mimir/mimir.sock`) — planned for local CLI (see #25; not yet implemented)
2. **TCP** (`127.0.0.1:8080`) — fallback for remote clients, web UI, and Windows

### Daemon-down Handling

When a CLI command cannot connect to the daemon, the user is prompted:
```
Error: Mimir is not running.
Start the server now? [y/N]:
```
If the user agrees, the daemon is started and the command is retried.

### System Diagram

```
┌─────────────────────────────────────────────────────────┐
│                     User Interfaces                       │
│  (CLI, Chat UI, WebSocket, Proactive Notifications)    │
└─────────────────────────────────────────────────────────┘
                           │
                    Unix socket / TCP
                           │
┌─────────────────────────────────────────────────────────┐
│                     Core Agent (Daemon)                  │
│  (Input Router, Context Manager, Response Synthesizer) │
└─────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────┐
│              Subsystems (all in Rust)                    │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │  Reasoning   │  │  Knowledge   │  │  Proactive   │  │
│  │   Engine     │  │    Graph     │  │    Agent     │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │  Connectors  │  │   memory.md  │  │    Vision    │  │
│  │  Framework   │  │ (Working    │  │  Tracking    │  │
│  └──────────────┘  │   Memory)    │  └──────────────┘  │
│                     └──────────────┘                     │
└─────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────┐
│                     Storage Layer                        │
│         SQLite (local-first, single file)                │
│  - Knowledge Graph (entities, facts, temporal data)     │
│  - memory.md (hot cache, 2,500 char limit)              │
│  - Audit logs, patterns, preferences                    │
└─────────────────────────────────────────────────────────┘
```

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust (edition 2024) |
| Async Runtime | tokio |
| HTTP Server | axum |
| HTTP Client | reqwest |
| Database | SQLite (sqlx) |
| Config | TOML (serde + toml) |
| CLI | clap |
| Serialization | serde |
| Logging | tracing |
| LLM API | OpenAI-compatible (any provider) |

---

## Key Design Decisions

### Single Binary, Library Crates

The workspace produces one binary (`mimir`) but uses library crates for code organisation:
- `mimir-core` — shared domain logic (used by both daemon and tests)
- `mimir-server` — HTTP API layer (library, no binary). `AppState` construction is decomposed into per-subsystem init helpers in `state/builder.rs` (`init_context_manager`, `init_tool_registry`, `init_knowledge_graph`, `init_job_queue`, `init_agent_runtime`, `init_scheduler`, `init_connector_framework`) composed by `from_config_with_llm` in a fixed startup order (issue #265).
- `mimir-client` — HTTP client for CLI commands (library, no binary)
- `mimir-knowledge` — SQLite knowledge graph (Phase 2)
- `mimir-api-types` — shared serde wire types
- `mimir-connectors` — service ingestion framework; connectors normalize external data into KB facts via the `KnowledgeGraph` facade, no direct `sqlx` (Phase 3, in progress). The `connectors` instance-registry table + facade methods (`list_connectors`, `get_connector_by_slug`, `upsert_connector`, `update_sync_cursor`, `set_connector_status`, `set_auth_state`) landed in #179 / F2; the `sources.connector_instance_id` provenance FK + per-connector item-count query landed in #180 / F3; the shared `normalize_and_insert(kg, Vec<NormalizedFact>, Provenance) -> ExtractionOutcome` boundary (provenance-annotated `NormalizedFact` exported from `mimir-knowledge`) landed in #181 / F4, so both chat `remember` extraction and connectors funnel through one deterministic resolve → confidence → sensitivity-gate → insert pipeline (corroboration/supersession/inference inherited from `insert_fact_in_tx`), and every insert path (`insert_fact`, `insert_facts_batch`, and the pipeline itself) reads connector confidence from the `connector_reliability` table via the single `confidence::connector_reliability` helper, so adjusted reliability scores reach the connector pipeline (issue #292); the full entity-resolution chain (exact name → alias → FTS5 fuzzy ≥ 0.9 → create, type-filtered) landed in #182 / F5; and the runtime async, object-safe `Connector` trait + data types (`ConnectorMode` polling/push, `SyncOptions`/`SyncOutcome`, `HealthStatus` transient probe, `ConnectorAction`/`ActionResult`, `ConnectorError`) landed in #183 / F6 — two-step DB-free ingestion (`sync` fetches raw → `extract` produces `NormalizedFact`s → the supervisor calls `normalize_and_insert`) — and the `ConnectorRegistry` + multi-backend factory dispatch landed in #184 / F7: the registry maps `(connector_type, backend)` to a `ConnectorFactory` (`register`/`register_arc`/`create`/`factory`/`backends_for`/`registered_types`; fail-loud duplicate detection; `FnConnectorFactory` closure helper + always-compiled `MockConnectorFactory`), with reliability staying per-type. The `ConnectorSupervisor` supervised lifecycle landed in #185 / F8: it owns one supervised background task per connector whose status is `Active` and centralises spawn-on-startup, restart with exponential backoff, a circuit breaker (after `max_failures` consecutive failures → `status = Error`, stop auto-restarting, manual `resume` required), auth-expiry pausing (`health() == AuthExpired` → `auth_state = Expired`, `status = Paused`, task exits), graceful shutdown (observes the shared `watch::Receiver<bool>` shutdown channel; aborts in-flight cycles), and cursor persistence (`update_sync_progress_and_durable_state` after extraction, deletion trashing, and fact insertion succeed, so a supervisor shutdown mid-cycle resumes from the last completed sync on restart; failure-safe in-memory cursor adoption landed in #314: the supervisor hands the persisted cursor back to the connector via `Connector::on_cycle_succeeded` only after a fully successful cycle, so the Calendar connector's in-memory `sync_token` never advances inside `sync` and a cycle that fails after fetching re-processes its window on the next in-process cycle. The Email and Photos connectors adopted the same pattern in #332: the Email connector's in-memory `last_uid` no longer advances inside `run_sync` (its durable LLM-extraction retry ledger only covers LLM-layer failures inside `extract`, so a hard extract/insert/persist failure re-fetches the failed window from the last confirmed cursor), and the Photos connector's scan/event passes return the computed cursor without adopting it, with the next in-process `sync` re-scanning the watch directory from the last confirmed cursor when a previous cycle failed (the file watcher does not re-deliver consumed events) — daemon `AppState` wiring landed in A1 / #202 (the daemon constructs and owns the `ConnectorRegistry` + `ConnectorSupervisor`, restores `Active` runners at startup, and drains them on graceful shutdown via `AppState::shutdown`)). `Paused`/`Error`/`Setup` rows are not auto-spawned. Each cycle runs `health` → `sync` → `extract` → `normalize_and_insert` in an isolated sub-task so a connector panic is caught via `JoinError::is_panic` instead of unwinding the runner; polling connectors sleep `interval + jitter` between cycles, push connectors block in `sync`. `SupervisorConfig` (`max_failures`, `base_backoff`, `max_backoff`) is injected at construction (no env mutation); `yield-on-user-activity` is deferred for V1. This is a library component in `mimir-connectors` with integration tests; daemon `AppState` wiring landed in A1 / #202 (registry + supervisor owned by `AppState`, connector CRUD/status routes `GET/POST /connectors`, `GET/DELETE /connectors/{id}` with derived item counts). Connector action routes + OAuth token ingest + the `forget` cascade landed in A2 / #203 (v0.92.0): `POST /connectors/{id}/sync` (manual sync), `/pause` / `/resume` (lifecycle), `/tokens` (credential ingest + `auth_state` flip), `/actions` (write-back dispatch), and `/forget` (cascade-forget sourced facts, secret, and row); backed by new `ConnectorSupervisor::start`/`pause`/`resume`/`act` methods (each `ConnectorHandle` retains the live `Arc<dyn Connector>`) and `KnowledgeGraph::forget_connector_facts`. The `mimir connector` CLI landed in A3 (#204, v0.95.0): add/auth/list/status/sync/pause/resume/remove/forget/act subcommands plumbing the routes through `mimir-client`, with `key=value` config pairs, non-OAuth credential prompts, `--json` output, and an e2e cycle test against an in-process daemon (mock-connector feature). Non-OAuth secrets can be supplied without leaking to the process list or shell history via `--password-stdin`/`--token-stdin` (the whole piped stream, one trailing newline stripped) or the `MIMIR_CONNECTOR_PASSWORD`/`MIMIR_CONNECTOR_TOKEN` env vars (read by the CLI only, never the daemon); per-kind precedence is flag → stdin flag → env var → interactive prompt, and `auth` infers the credential kind from the env vars when no config/flag declares one (#270, v0.113.0). The interactive OAuth PKCE loopback flow landed in A4 (#205, v0.97.0): `mimir connector add` / `auth` with an `auth.kind=oauth` config run the flow in the CLI process — `mimir-connectors::oauth::pkce::run_pkce_flow` binds an ephemeral loopback listener on `127.0.0.1:0`, opens the provider's authorize URL in the browser (printed first for headless/SSH sessions) with an S256 PKCE challenge + CSRF state, receives the redirect (8 KiB read cap, state validated, favicon probes ignored), exchanges the code via the shared `OAuthHttpClient` (HTTPS/loopback token-endpoint gate + secret-hygiene error mapping), and POSTs the resulting `SecretBundle::OAuth` to the daemon's token-ingest route so the instance becomes `authenticated` — the daemon never runs a transient HTTP server. The flow runs before the instance is registered in `add`, so a canceled flow exits with nothing created; `auth` re-runs it for expired credentials, taking the OAuth client config from re-supplied `key=value` / `--config-json` args (the daemon does not expose the stored config on the wire). `CalendarAuthMethod::OAuth` / `EmailAuthMethod::OAuth` gained a required `auth_uri` field (breaking config change) so the flow knows the provider's authorization endpoint. The secret store (F10) landed (#187 / F10). The first concrete backend landed: the local-filesystem **Photos** connector (#195 / C1) — a read-only, push-mode `notify` recursive watcher (debounced ~2s) with `kamadak-exif` EXIF GPS + datetime extraction, emitting one fact per photo — C1 a coords-only `took_photo` fact, C2 (#196, v0.81.0) a `took_photo_at <place>` fact where EXIF GPS is reverse-geocoded to a locality-level place name via the shared `Geocoder` (injected through a new `ConnectorContext` threaded factory → registry → supervisor; a coord-dedup cache bounds geocode calls to one per ~111 m shooting spot) — with a per-file mtime/inode incremental cursor. Photos are facts, not entities: the place is a `Place` object entity, photos at the same place corroborate into one open-ended fact (+0.05/source, capped 0.95), and a new idempotent `Geographic` `entity_locations` row (`LocationType::Geographic = 6`, migration `046`) anchors the place's own coordinates so `find_nearby` resolves places by where they are. When no place resolves, the photo degrades to the C1 coords-only shape so no data is lost. Knowledge-graph growth is O(distinct places), not O(photos). The supervisor now injects the persisted `sync_cursor` into a connector's `config_json` as `__cursor` so incremental connectors can skip already-processed files across restarts. The second backend, the CalDAV **Calendar** connector, landed in #197 / C3 (v0.82.0): transport (PROPFIND + sync-collection REPORT, sync-token incremental sync, `icalendar` VEVENT parsing, app-password + OAuth-refresh auth). C4 (#198, v0.84.0) adds event → KB fact extraction: `extract()` drains VEVENTs into a fact cluster (`user has_event <event>` typed `Appointment` + recurrence from `RRULE` `FREQ`, `<event> located_in <place>`, `<attendee> attending <event>`) resolved via the full F5 chain, with future-dated/recurring events surfacing in the user's Upcoming section; dates parse to UTC via `chrono-tz` (TZID-qualified included); and the only connector write-back — `act()` creating/updating/deleting remote events via CalDAV `PUT`/`DELETE`. The connector authors facts as the canonical `[identity] name` injected via `ConnectorContext::user_identity` (matching the daemon's `user_entity_id`); `NormalizedFact` gained an `event_type: Option<EventType>` hint (chat stays `Task`/`Reminder`). Server-side deletion → KB fact lifecycle is a follow-up. The third backend, the IMAP **Email** connector, landed in #199 / C5 (v0.83.0): transport-only (`async-imap` over a hand-rolled TCP+rustls handshake — `LOGIN`/`AUTHENTICATE XOAUTH2`, `IDLE` push with a polling fallback auto-detected via CAPABILITY, `UID FETCH` incremental sync with a UIDVALIDITY-safe `<uid_validity>:<last_uid>` cursor, `BODY.PEEK[]`); the OAuth refresh is shared with the Calendar connector via the `oauth` module (DRY; since #240 / v0.96.0 the refresh runs on the vetted `oauth2` crate (5.0.0, `default-features = false`) through a custom `OAuthHttpClient` adapter over the workspace's single reqwest 0.13 client — no reqwest 0.12 in the tree, redirects disabled, the HTTPS/loopback endpoint gate and secret-hygiene error mapping preserved, and the expiry-check + refresh + persist decision logic DRY-extracted into `oauth::resolve_access_token`). Mail → structured fact extraction landed in #200 / C6 (v0.85.0): `extract()` runs a deterministic extraction cascade over staged RFC 822 messages and, today, turns iMIP calendar invites (`text/calendar; method=REQUEST|REPLY`) into the same appointment fact cluster the Calendar connector emits, reusing a new shared `mimir-connectors::ical` module (the Calendar connector's VEVENT parsing + fact cluster, now DRY across both backends, gated `any(feature = "calendar", feature = "gmail")`). The email is treated as provenance (its IMAP UID is each fact's `raw_reference`), not the fact — no per-email communication facts and no `Person` entities auto-created from `From`/`To` headers, so marketing/spam produces no junk; a plain prose email with no `text/calendar` part produces nothing. The connector now authors user-scoped facts against the injected `ConnectorContext::user_identity` (matching the Calendar connector). Deterministic `schema.org` JSON-LD extraction for transactional email is #249 (v0.86.0). LLM extraction for free-text prose a deterministic layer cannot read is C7 (#201, v0.88.0): `extract()` runs a third, last-resort layer that, for messages layers 1 (iMIP) and 2 (JSON-LD) produced no facts for, calls the shared `LlmBackend` under a strict `extract_email_facts` tool schema on the `LlmWorkerPool`'s **system queue** (new `LlmBackend::system_chat_message` trait method, priority below user chat, now carrying tool schemas) so a one-call-at-a-time provider is never starved by extraction and a queued user chat preempts a waiting connector call; Rust validates every field against the typed enums before building `NormalizedFact`s (`event_type` mapped against the `EventType` enum, dropped if unrecognised), reusing the shared `mimir-knowledge::extract` parsing helpers (DRY with the conversational `remember` path); obvious bulk-marketing mail is skipped by a deterministic Rust `is_likely_spam` pre-filter (a `List-Unsubscribe` header, or a `From` domain on a pure marketing platform — general-purpose ESPs that also deliver transactional mail are kept without the unsubscribe signal) before any LLM call; a retryable LLM/parse failure is propagated as a `ConnectorError` and the raw email is re-staged for retry on the next extraction cycle. This change also resolves #234: `ExtractionMethod` moves onto `NormalizedFact` as a per-fact override (defaulting to the batch `Provenance`'s method), so a single mixed-method `extract()` batch records the right method per fact in `sources.extraction_method_id` (the supervisor no longer hardcodes `StructuredParse`). The `Arc<dyn LlmBackend>` reaches the connector via a new `ConnectorContext::llm_backend` (+ `ConnectorSupervisor::with_llm_backend`); daemon wiring landed in #202 / A1 (the supervisor is wired with `with_llm_backend` at startup). The Photos GPS→place enrichment landed in #196 / C2 (v0.81.0). Manual sync triggering landed in #186 / F9: `ConnectorSupervisor::trigger_sync(id, SyncOptions)` (and `trigger_sync_by_slug`) preempts a connector's polling interval with caller-supplied options (`full` forces a non-incremental pass, `since` is a relative window), serialises concurrent triggers per connector via a one-permit `Semaphore` + request channel (overlapping triggers queue, never run in parallel), and returns the cycle's `TriggerOutcome` (`Ok { fetched, new_cursor }`, `AuthExpired`, `Failed`); `TriggerError::NotRunning` for paused/errored/exited connectors, `TriggerError::PushUnsupported` for push-mode connectors (deferred). The runner's post-cycle wait is now a `select!` between the polling interval, a manual trigger, and shutdown, so a trigger preempts the interval (and backoff after a failure); `run_cycle` takes `SyncOptions` and `CycleOutcome::Ok` carries the `SyncOutcome`. The T1 integration/E2E harness landed in #206 (v0.98.0): `mimir/tests/connector_e2e.rs` configures the `gmail/test` mock connector's `facts` knob and drives add → auth → resume → sync through the real CLI + in-process daemon, then verifies the KB via `mimir kb query` / `kb show --json` — facts land with `source_type=Connector`, provenance tied to the instance (`connector_instance_id` + `raw_reference`), confidence from the connector reliability score (Gmail = 0.85), and a second instance corroborating the same claim boosts confidence to 0.90 while a plain re-sync stays a re-statement no-op; the supervisor-level round trip in `mimir-connectors/tests/mock_ingestion_e2e.rs` now asserts the exact 0.85 score, and `TestDaemon` gained a `run_cli_json` helper (DRY with the lifecycle e2e test). The shared fake-browser OAuth test doubles landed in #290 (v0.100.0): `mimir_connectors::test_utils` (feature `test-utils`, off by default) owns `self_callback_opener` / `parse_authorize_url` / `callback_url`, used by the PKCE unit tests and the CLI connector tests (previously two private copies of the same helper); the wiremock token-endpoint mock `mount_token_endpoint(server, expected_calls)` joined it in #298 (v0.111.0), replacing the five inlined copies of the token-response mock across the same two suites.
- `mimir` — binary crate, thin dispatcher

This avoids the problems of a two-binary architecture (separate `mimir` CLI and `mimir-server`):
- No fragile process spawning (old `mimir start` searched PATH for a second binary)
- No duplicated state (each binary had its own LlmClient, ContextManager, etc.)
- No coordination gap (separate processes with no shared state)
- Single systemd unit, single binary to install

### Daemon as the Single Source of Truth

All state lives in the daemon process. CLI commands are thin HTTP clients:
- `mimir ask` → `POST /chat` or `POST /chat/stream`
- `mimir chat` → interactive SSE client
- `mimir status` → `GET /status`
- `mimir memory` → `GET /memory`
- `mimir stop` → `POST /shutdown` (or SIGTERM via systemd)

### Unix Domain Socket Transport

The daemon will listen on both a Unix domain socket and a TCP socket once UDS is implemented (see #25). Currently, only TCP localhost (`127.0.0.1:8080`) is active. The CLI will prefer the Unix socket (faster, more secure, instant daemon detection) and fall back to TCP (for remote clients, web UI, Windows) when UDS is available.

### systemd Integration

`mimir start` runs the daemon in the foreground. systemd manages backgrounding, restart-on-failure, and logging. `mimir init` offers to install the systemd user service. See `VISION/08-Architecture/Deployment-Model.md`.

---

## Personality System

- **Presets:** `transparent` (default), `concise`, `warm`, `formal`
- **System prompt:** Generated from personality preset + memory.md content
- **Override:** `mimir ask -p concise "..."` or `mimir chat` then `/personality concise`
- **Extensible:** Custom personalities can be added via config in future versions

---

## Memory System

### memory.md (Working Memory)
- ~2,500 character budget (~900 tokens)
- Injected into every system prompt for fast context
- Auto-managed: add, replace, remove entries
- Frozen per session (snapshot taken at start)
- Persisted to disk immediately on change

### Context Manager
- SQLite-backed session and message storage
- Sliding window of recent conversation
- Token-aware trimming (removes oldest pairs first)
- Cumulative token usage tracking per session

---

## Tool & Skill System

### Tools
- Object-safe `Tool` trait: `name()`, `description()`, `parameters_schema()`, `execute()`
- Three permission levels: `Auto` (always run), `Ask` (confirm first), `Deny` (never run)
- Built-in: `echo`, `get_current_time`
- CLI wrappers: invoke external commands as tools

### Skills
- Object-safe `Skill` trait with `SkillContext` (access to tools, LLM, context)
- Three sources: `Builtin`, `User` (Markdown files), `Generated` (auto-created)
- Metrics tracked in SQLite: invocation count, success rate, user corrections
- Phase B (issue #20): system-generated skills, utility scoring, pruning, promotion

---

## Proactivity System (Phase 5)

- **Trust ladder:** Observation → Gentle Offers → Pattern Permissions → Autonomous
- **Proactivity levels:** `never`, `important_only`, `always`
- **Notification fatigue detection:** If 3+ dismissals in a row, pause proactivity.

---

## Knowledge Graph (Phase 2)

- SQLite-based, single file, local-first.
- Entities, Facts (directed temporal edges), Sources (provenance), Preferences.
- Temporal facts: `valid_from`, `valid_until`. History is preserved.
- Confidence scores: 0.0-1.0. Facts color-coded by confidence.
- **Corroboration (#79):** inserting a non-explicit fact that covers the same claim as an existing Active fact (same subject+predicate+object, temporally overlapping) adds a source to the existing fact (no new row) and boosts its confidence +0.05 per independent source, capped at 0.95 (explicit/inferred facts excluded). Re-statements from the same source are a no-op. Runs inside `insert_fact_in_tx`, before supersession.
- **Entity locations (#193/#194/#196/#228):** a "where" fact carries a typed `NormalizedLocation` overlay that `normalize_and_insert` turns into an `entity_locations` row (address + lat/lng + timezone + temporal bounds) for the resolved subject, with the missing geo half filled by the injected `Geocoder`. Location types: `Home`, `Work`, `Visited`, `Origin`, `Current`, and `Geographic` (a place entity's own coordinates, anchored idempotently by the Photos connector in C2 / #196). Moves supersede prior open-ended locations of the same type; same-place re-statements (matching address or coordinates within 0.1 km, no conflicting shared attribute, overlapping period) fold into the existing row instead of duplicating it, while disjoint periods of the same place stay separate rows (#228). `find_nearby` is a bounding-box pre-filter + exact Haversine post-filter with optional temporal scoping. A background overlay worker drains location jobs serially (geocoder-rate-limit-safe).
- Obsidian-compatible export/import (Markdown + YAML frontmatter + wiki-links).
- Nightly optimization: deduplication, contradiction resolution, dormant cleanup.
- **Events & reminders (#74, v0.57.0):** a lifecycle + recurrence overlay on facts. A future-dated fact is a one-time event; a recurring fact (e.g. a birthday) is a recurring event; a `requires_user_action` fact is a task. An `events.upcoming_scan` job (default 06:00 & 18:00) derives overlays, auto-completes past one-time events, and advances recurring events. Upcoming events surface in the "Upcoming" memory section. `entity_dates` is deprecated and removed (replaced by this overlay; recurrence logic moved to `models::recurrence`).

---

## Phase 1: Core Agent (Current Plan)

**Goal:** Build the foundational layer. The agent can start, hold a conversation, stream responses from an OpenAI-compatible endpoint, and manage memory.md.

**Key deliverables:**
- Single `mimir` binary with daemon and client modes
- CLI commands talk to daemon via HTTP (Unix socket preferred, TCP fallback)
- Daemon-down detection with user prompt to start
- systemd integration for auto-start
- LLM streaming, context management, personality presets
- Tool and skill registries

See `VISION/09-Roadmap/Phase-1-Core-Agent.md` for full task list.

---

## Phase 2+: Roadmap Summary

| Phase | Focus | Duration | Key Deliverables |
|-------|-------|----------|-----------------|
| 1 | Core Agent | 4-6 weeks | Single binary, daemon/client, CLI, chat, LLM, memory.md |
| 2 | Knowledge Graph | 4-6 weeks | SQLite schema, entities, facts, temporal queries |
| 3 | Connectors | 6-8 weeks | Gmail, Calendar, Photos, normalization pipeline |
| 4 | Reasoning Engine | 6-8 weeks | Multi-thread investigation, meta-threads, streaming |
| 5 | Proactive Agent | 4-6 weeks | Event monitoring, pattern recognition, trust ladder |
| 6 | Vision Tracking | 6-8 weeks | Object detection, spatial memory, re-identification |

---

## Important Files and Locations

### Config
- User config: `~/.config/mimir/config.toml`
- Default config: `config/default.toml`
- memory.md: `~/.config/mimir/memory.md`
- Data: `~/.local/share/mimir/`
- Socket: `~/.local/share/mimir/mimir.sock`

### Key VISION Docs (if you need to reference)
- `VISION/00-Overview/Vision-Statement.md` — Core premise and principles
- `VISION/01-Core-Agent/Personality.md` — Personality system
- `VISION/01-Core-Agent/Memory-System.md` — memory.md design
- `VISION/01-Core-Agent/Technical-Design.md` — Architecture, single binary design
- `VISION/01-Core-Agent/User-Experience.md` — CLI and daemon interaction
- `VISION/02-Knowledge-Graph/Learning-Modes.md` — Explicit vs casual learning
- `VISION/02-Knowledge-Graph/Temporal-Facts.md` — Temporal storage model
- `VISION/04-Reasoning-Engine/Technical-Design.md` — Investigation threads, meta-threads
- `VISION/05-Proactive-Agent/User-Experience.md` — Trust ladder
- `VISION/08-Architecture/Deployment-Model.md` — systemd, single binary, Unix socket
- `VISION/08-Architecture/Permission-Model.md` — Permission levels

---

## How to Start Implementing

1. **Clone the repo:** `git clone https://github.com/BhavsarDevansh/Mimir.git`
2. **Read the Phase 1 roadmap:** `VISION/09-Roadmap/Phase-1-Core-Agent.md`
3. **Start with the mono-binary consolidation** (current work)
4. **TDD throughout** — every feature starts with a failing test

---

## Environment Prerequisites

- Rust toolchain (latest stable, edition 2024)
- SQLite development libraries
- An OpenAI-compatible API key (or local model endpoint)
- Git

---

## Success Criteria for Phase 1

- [x] `cargo build --workspace` succeeds
- [x] `cargo test --workspace` passes
- [x] `mimir start` runs the daemon in the foreground (no separate binary)
- [ ] `mimir ask "hello"` talks to the daemon via HTTP (tracked in #30)
- [ ] `mimir chat` starts an interactive session via the daemon (tracked in #30)
- [ ] `mimir status` queries the daemon for health (tracked in #30)
- [ ] `mimir stop` signals the daemon to shut down (tracked in #31)
- [ ] Daemon-down prompt: CLI asks user if they want to start the daemon (tracked in #33)
- [x] SSE streaming endpoint works for chat
- [ ] systemd user service works for auto-start (tracked in #34)
- [ ] `mimir-client` crate exists and is a workspace member (tracked in #30)
- [ ] Conversation history display works in `mimir chat` (tracked in #36)
- [ ] Markdown responses are preserved in terminal output (tracked in #36)
- [ ] End-to-end round-trip test passes (tracked in #35)
- [ ] Config hot-reload works for non-sensitive settings (tracked in #32)

---

*End of Implementation Context*
