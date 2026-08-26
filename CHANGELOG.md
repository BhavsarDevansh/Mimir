# Changelog

## [0.152.1] — 2026-08-26

### Fix: PR #513 review round 2 — recurrence constraints, range time zones, and series retirement (issue #474)

- The events-subsystem occurrence engine now evaluates the stored `RRULE` day/month constraints instead of advancing only by kind/interval: `BYDAY` selects the weekdays of a weekly series (multi-day weekly events advance to the next constrained weekday, respecting `INTERVAL` weeks), `BYMONTHDAY` the day of an absolute monthly/yearly series, `BYMONTH` the month of a yearly series, and `BYDAY` + `BYSETPOS` the Nth weekday of a relative monthly/yearly series (including `last`). `next_occurrence` takes the raw rule, and occurrence-level tests cover multi-day weekly, fortnightly multi-day, absolute/relative monthly, and absolute/relative yearly patterns.
- The Graph `endDate` range now preserves `recurrenceTimeZone`: `UNTIL` is the inclusive local end-of-day (`23:59:59`) in the range's time zone (falling back to the event time zone, then UTC), converted to UTC — a zone ahead of UTC no longer leaks the next local date into the series and a zone behind UTC no longer truncates the last local day.
- The upcoming scan retires a recurring overlay when its series ends: when `next_occurrence` returns `None` (the next occurrence would fall past `recurrence_until`, or the rule no longer yields one), the overlay transitions to `Completed` so the scan stops selecting it on every cycle and it never surfaces as overdue. Regression tests cover a past final occurrence and rule-driven advancement through the scan.
- Docs updated (`docs/events-reminders.md`, `docs/calendar-connector.md`, `docs/wiki/calendar-connector.md`, `Mimir-Implementation-Context.md`).
- Version bumped 0.152.0 → 0.152.1 (patch — backwards-compatible bugfixes).

## [0.152.0] — 2026-08-26

### Feature: Microsoft Graph calendar backend for Outlook / Office 365 (issue #474)

- New `(Calendar, graph)` backend in `mimir-connectors/src/calendar/graph`, registered in the existing multi-backend factory (F7 / #184 — a new factory registration, no database change): `GraphCalendarConnector` + `GraphCalendarConnectorFactory` sync the user's default calendar through the Microsoft Graph events delta query (`GET /me/events/delta` with `@odata.nextLink` paging and the final `@odata.deltaLink` as the opaque supervisor cursor, reusing the failure-safe `on_cycle_succeeded` cursor handoff from issue #314). `@removed` events are staged as tombstones keyed by the event id and propagated to the KB fact lifecycle via `extract_deletions` (issue #247); HTTP 401 maps to `NotAuthenticated` so the supervisor's one-shot forced-refresh retry (issue #507) runs before pausing.
- Deterministic event → facts construction with no LLM extraction: each Graph event maps onto the shared `RawVEvent` shape (`subject`, `start`/`end` resolved to UTC via `chrono-tz` with the iCal parser's unknown-zone fallback, `location.displayName`, attendee display names, and the recurrence pattern type mapped to an RRULE `FREQ`) and delegates to the shared `crate::ical::vevent_to_facts`, so the backend authors the same cluster as CalDAV — `user has_event <event>` (typed `EventType::Appointment`, recurrence), `<event> located_in <place>`, `<attendee> attending <event>` — with the Graph event id as `raw_reference`.
- OAuth 2.0 only (app-password configs are rejected at construction with a clear error), via the existing PKCE loopback flow (A4 / #205) and connector-side refresh (`oauth::resolve_access_token`, issue #240) with the scope `https://graph.microsoft.com/Calendars.Read offline_access`; the user brings their own app registration (Mimir has no public client ID). The transport never follows redirects and origin-checks every server-supplied `@odata.nextLink`/`@odata.deltaLink` against the configured service root (default `https://graph.microsoft.com/v1.0`, overridable via `base_url` for national clouds), so a compromised response cannot redirect the bearer token.
- Wizard: the `(calendar, graph)` profile is an Outlook / Office 365 preset that asks which Microsoft account type the app registration targets and pre-fills the matching identity-platform authorize/token endpoints (reusing the #467 endpoint mapping — `/consumers/`, `/organizations/`, `/common/`, or tenant-specific endpoints for single-tenant registrations — never a hardcoded `/common/` trap) plus the Graph calendar scope; the stored token endpoint matches the registration's audience by construction.
- Read-only: no `act()` write-back (Graph is read-only; CalDAV remains the only write path). Cancelled events (`isCancelled`) are not treated specially yet — the same gap the CalDAV backend has for `STATUS:CANCELLED`.
- Delta-reset self-healing: an expired or server-invalidated delta token (the Graph contract answers `410 Gone`, or `400` with the `syncStateNotFound` error code) restarts the sync with a full synchronization in the same cycle, and a delta response without a final `@odata.deltaLink` clears the in-memory cursor so the next cycle re-syncs from scratch — a stale cursor never wedges the connector in a permanent failure loop.
- Tests: 19 new unit tests in `mimir-connectors/src/calendar/graph/tests.rs` (delta full/incremental sync, paging, `@removed` tombstones, 401 mapping, foreign-link rejection, health probe, sync → extract fact cluster with recurrence + timezone mapping, tombstone staging/acknowledgement, failure-safe cursor handoff, missing-deltaLink full-resync, delta-reset self-healing, OAuth refresh-on-expiry with bundle persistence, `authenticate` 401 → `Expired`, app-password config rejection) plus 3 wizard tests pinning the account-type prompt, the endpoint/scope pre-fill, and the single-tenant flow; docs updated (`docs/calendar-connector.md`, `docs/connector-management.md`, `docs/cli.md`, `docs/wiki/calendar-connector.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, `VISION/09-Roadmap/Phase-3-Connectors.md`, `Mimir-Implementation-Context.md`).
- Recurrence fidelity (PR #513 review): the Graph recurrence pattern + range now map onto a full RRULE (`FREQ` + `INTERVAL`, `BYDAY`/`BYMONTHDAY`/`BYMONTH`/`BYSETPOS`, and `COUNT`/`UNTIL`), and the shared VEVENT extractor parses the interval and series bounds into the fact and its event overlay — migration 058 adds `events.recurrence_rule` / `recurrence_interval` / `recurrence_until` (and the same columns to `pending_event_meta`, so sensitive recurring facts keep their interval and bounds across confirmation) and `next_occurrence` steps by the interval and stops at the effective series end, so a fortnightly event advances every two weeks and a bounded series no longer stays active indefinitely (fixtures cover fortnightly, relative-monthly, and bounded series).
- Delta paging is bounded (PR #513 review): `sync_events` caps the `@odata.nextLink` chain at 1,000 pages and fails with a clear error, so a server that repeats a `nextLink` cannot spin the loop forever.
- Staging dedup (PR #513 review): live events are staged into the buffer keyed by Graph event id (latest version wins), so a cycle cancelled after `sync` but before `extract` re-stages the same delta without duplicating fact clusters.
- Wizard DRY (PR #513 review): the Microsoft account-type endpoint prompt is extracted into one `microsoft_account_endpoints` helper shared by the Email IMAP and Graph calendar profiles.
- Docs (PR #513 review): the read-only limitation is scoped to the Graph backend (CalDAV remains the only write-capable connector), the mock factory's `mock-connector` feature gate is documented, the connector inventory counts three connector types / four registered backends, and the feature-matrix metadata matches 0.152.0.
- Version bumped 0.151.2 → 0.152.0 (minor — backwards-compatible new feature).

## [0.151.2] — 2026-08-26

### Fix: Email LLM extraction no longer drops facts invisibly (issue #508)

- The email prose-extraction system prompt now lists the full canonical predicate vocabulary — rendered from the same `mimir_knowledge::CANONICAL_PREDICATES` const the Rust validator checks, plus the open `favourite_<thing>` family — and the tool schema's `relationship_type` description states that any other predicate is dropped, so the model sees the exact words it may emit instead of inventing near-miss predicates that are then discarded. The rendered vocabulary is a `LazyLock` static (one allocation per process), and a test pins the prompt to the validator in both directions.
- LLM-layer drops are now counted, not just logged: `extract_prose_facts` returns a `ProseExtractionOutcome` carrying the validated facts plus the drop count, the `connector_item.remember` hook logs a per-email `LLM email extraction dropped N of M facts` warning, and migration `057` adds cumulative `facts_accepted` / `facts_dropped` columns to `connectors`, updated after each successful extraction via `KnowledgeGraph::record_connector_fact_counts`.
- `mimir connector list` / `status` now surface the counters (`accepted` / `dropped` columns and `Facts accepted:` / `Facts dropped:` detail lines), so a vocabulary regression like the 2026-08-24 outlook backfill — 247 dropped facts hidden behind `items: 14` — is visible instead of silent. Connectors that do not run the LLM layer report 0.
- Tests cover the prompt/validator vocabulary pinning, the dropped-fact count on `extract_prose_facts`, the hook's counter writes (accepted and dropped) on a mixed extraction, and the `record_connector_fact_counts` increment/not-found semantics; docs updated (`docs/email-connector.md`, `docs/connector-management.md`, `docs/wiki/connectors.md`, `docs/wiki/email-connector.md`).
- The alias-mapping direction (mapping invented predicates onto canonical ones) is deliberately left to the open redesign issue #468, which absorbs the full relation-tree + approval-flow design; this change stops the silent loss and gives the model the vocabulary, without pre-empting that design.
- Version bumped 0.151.1 → 0.151.2 (patch — backwards-compatible bugfixes).

## [0.151.1] — 2026-08-26

### Fix: PR #511 review — OAuth ingest validation and confidential-client re-auth

- The token-ingest route (`POST /connectors/{id}/tokens`) now rejects an OAuth bundle whose non-secret `config` slice declares a non-OAuth kind (`app_password` / `api_token`) with a `400` before persisting anything — previously it stored incompatible credentials + config and reported `Authenticated`, only to fail credential-kind resolution at the next connector construction.
- Stored-config OAuth re-authentication now merges re-supplied OAuth fields onto the stored non-secret metadata instead of discarding them: a confidential client that re-supplies only `auth.client_secret=...` keeps the stored endpoints / client id / scopes, the secret reaches the PKCE exchange and the credential bundle, and `oauth_config_slice` still excludes it from persisted `config_json`.
- Tests cover both fixes: a route test asserting the mixed-kind request is rejected with nothing persisted, and a CLI regression test proving the re-supplied client secret is used in the PKCE exchange (HTTP Basic auth) while never landing in `config_json`.
- Version bumped 0.151.0 → 0.151.1 (patch — backwards-compatible bugfixes).

## [0.151.0] — 2026-08-26

### Fix: OAuth connector re-auth and auth-expiry retry (issue #507)

- `mimir connector auth <slug>` can now re-auth an OAuth connector without re-supplying the OAuth fields: the daemon reads the stored non-secret auth config (`ConnectorResponse.auth` — kind, `username`, `auth_uri`, `token_endpoint`, `client_id`, `scopes`, with `client_secret`/passwords/tokens always stripped), and the CLI re-runs the PKCE flow from the stored endpoints when the stored kind is `oauth`; the interactive credential-kind prompt also offers an "OAuth 2.0" fallback that guides the user to the required config pairs when the stored config does not declare OAuth. Config-free re-authentication serves **public PKCE clients** — a confidential client re-supplies `auth.client_secret` with the config, and the secret is carried in the credential bundle and never persisted to `config_json`. The token-ingest route now persists the driving non-secret OAuth slice into `config_json` alongside the bundle, so a connector re-authed through the fallback declares OAuth before its next construction instead of failing with a credential-kind mismatch (issue #507 review).
- A single auth rejection no longer pauses an OAuth connector outright: the supervisor runs one forced refresh (`Connector::force_refresh`, bypassing the 60 s refresh-skew window via `resolve_access_token(..., force = true)`) and re-probes with the fresh credential; only a second rejection or an auth-level refresh failure (e.g. a revoked refresh token) pauses — a transient refresh failure (network / malformed response) is a recoverable cycle error that backs off and retries — and the pause now persists and logs the actual rejection message (IMAP `BAD`/`NO` text, `invalid_grant` description, or the CalDAV 401) as `last_error` instead of the generic "auth expired" — `HealthStatus::AuthExpired` carries the message end-to-end through `TriggerOutcome` and the `SyncConnectorResponse::AuthExpired { message }` wire shape, so `mimir connector sync` reports it too.
- Transient secret-store availability failures (`SecretError::Io` / `Keyring` / `KeyringTask`) at the forced-refresh boundary now map to a recoverable `ConnectorError::Network` (backoff + retry) instead of `ConnectorError::Authentication`, so an unavailable disk or OS keychain no longer pauses a connector as an auth rejection (issue #507 review).
- Tests cover the forced-refresh unit path (email + calendar + shared OAuth helper), the supervisor retry-once/pause-with-detail cycle outcomes, the daemon's sanitized auth-config response (list + read, secret-strip assertions) and its OAuth-config persistence on token ingest, the CLI stored-config PKCE re-auth against a mocked daemon, and the real daemon E2E re-auth round trip against the mock OAuth server; docs updated (`docs/cli.md`, `docs/connector-management.md`, `docs/connectors-framework.md`, `docs/oauth-client.md`, `docs/email-connector.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, `Mimir-Implementation-Context.md`).
- Version bumped 0.150.0 → 0.151.0 (minor — breaking public wire-type change: `SyncConnectorResponse::AuthExpired` gained its required `message` field, so consumers of the former unit variant must be updated; `IngestTokenRequest::OAuth` gained the optional `config` slice).

## [0.150.0] — 2026-08-25

### Feature: LLM semantic entity dedup + merge-queue review (issue #282)

- `enqueue_semantic_dedup` is implemented: candidate entity pairs are evaluated under a strict `evaluate_entity_dedup_candidates` tool schema, with Rust-side validation (pair membership in the candidate set, `merge`/`keep_separate` action enum, finite confidence in `[0,1]`), and every validated result is upserted id-ordered into `entity_merge_queue` with `suggested_action` + `llm_confidence` (new migration `055`), so the `UNIQUE(primary, duplicate)` constraint can never be bypassed and re-evaluation enriches pending rows instead of duplicating them.
- The nightly optimization pipeline gained the `entity_semantic_dedup` pass (11 passes total): a capped (50) deterministic pre-filter — same-type entities sharing an alias or with equal/contained names, excluding pairs the LLM already evaluated — feeds the LLM, and results land in the review queue only; entities are never auto-merged (migration `056` adds the `entity_merges_queued` pass-run counter).
- New review surface: `mimir kb merges list [--json]` shows pending suggestions (loopback-gated `GET /kb/merges`), `mimir kb merges apply <id>` runs the existing `auto_merge_pair` entity-merge logic (`POST /kb/merges/{id}/apply`, returns the actual survivor/merged ids), and `mimir kb merges keep <id>` marks a pair `KeptSeparate` (`POST /kb/merges/{id}/keep`).
- Tests cover LLM output validation, queue writes, no-duplicate/enrichment guarantees, pair-order normalisation, merge application, keep resolution, the nightly pass, and the candidate cap; docs updated (`docs/nightly-optimization.md`, `docs/knowledge-graph-schema.md`, `docs/cli.md`, `docs/inference-engine.md`, wiki, `Mimir-Implementation-Context.md`).
- Version bumped 0.149.3 → 0.150.0 (minor — new feature).

## [0.149.3] — 2026-08-25

### Docs: Align compaction reload contract and scope summary guarantees (PR #505)

- `docs/config-system.md`, `docs/wiki/context-manager.md`, and `docs/wiki/configuration.md` now state one reload contract: `Config::normalise` clamps the compaction window on load and reload, the synchronous compact-before-trim path reads the live (reloaded) values, and the background `session.compaction` hook is registered at daemon startup so its window and enablement change only after a restart.
- `docs/context-manager.md`, `docs/wiki/context-manager.md`, and `docs/wiki/what-works-now.md` now scope the "never dropped without a summary" guarantee to the hard `max_turns` trim: token-budget (`max_tokens`) trimming is not preceded by compaction and can drop the oldest turns without a summary.
- Version bumped 0.149.2 → 0.149.3 (patch — documentation update).

## [0.149.2] — 2026-08-25

### Docs: Compaction window invariant documented on the config field (PR #505)

- `ContextCompactionConfig::max_turns` now documents that `Config::normalise` clamps it strictly below `context.max_turns` after TOML and environment overrides are applied, making the PR #505 review fix traceable at the field definition.
- Version bumped 0.149.1 → 0.149.2 (patch — documentation update).

## [0.149.1] — 2026-08-25

### Fix: Session compaction review fixes (PR #505)

- The hard `max_turns` trim is now compaction-aware: both chat request paths run the compaction synchronously before `trim_to_budget` deletes turns, so a burst that outruns the idle-gated `session.compaction` hook still writes the removed turns to `sessions.summary` (no silent drops at the ceiling). The compaction window is also validated after TOML/environment overrides — an equal or inverted `context.compaction.max_turns` is clamped to one below `context.max_turns` on load and reload.
- The compaction summary is exported as a clearly labelled `user`-role context block instead of a `system`-role one, so potentially user-influenced summary text can never override the trusted system prompt. Transcript rendering escapes carriage returns and newlines in message content so an embedded line break cannot forge a false `role:` entry for the summarisation model.
- `apply_compaction` now deletes the summarised messages and writes the summary in one transaction (a single session-scoped `DELETE ... WHERE id IN (...)`, built with `QueryBuilder`), so a failure part-way can no longer leave a new summary alongside summarised messages that still exist.
- Added compaction tests covering turns with assistant tool calls plus tool results (preserved in the batch) and sessions ending on an in-flight assistant tool-call turn (kept out of the batch), plus an integration test that sends 25 turns during the idle cooldown and asserts the trimmed turns appear in `sessions.summary`.
- Docs updated: `docs/context-manager.md`, `docs/hooks.md`, `docs/config-system.md`, `docs/wiki/context-manager.md`, `docs/wiki/configuration.md`, `docs/wiki/what-works-now.md`, `Mimir-Implementation-Context.md`.
- Version bumped 0.149.0 → 0.149.1 (patch — review fixes).

## [0.149.0] — 2026-08-25

### Feature: Chat session compaction actually runs (issue #279)

- The `sessions.summary` / `compacted_at` columns were schema-only: nothing ever wrote them, so `trim_to_budget` silently discarded old context. A new idle-gated `session.compaction` hook now summarises the oldest complete turns via the LLM, stores the summary on the session, advances `compacted_at`, and deletes the summarised messages (same turn-boundary and in-flight-final-turn rules as trimming, shared via `split_complete_turns`).
- The summary is folded into the LLM conversation context on export, exposed on `GET /sessions` and `GET /sessions/{id}/messages`, and printed by the REPL `/history` resume flow as "Earlier context: …".
- New `[context.compaction]` config (`enabled` default true, `max_turns` default 15, env `MIMIR_CONTEXT_COMPACTION_ENABLED` / `MIMIR_CONTEXT_COMPACTION_MAX_TURNS`); the window sits below `context.max_turns` so compaction summarises turns before the synchronous trim, which remains the hard safety ceiling. Incognito sessions never compact (nothing is persisted). If the LLM summarisation fails, the compacted transcript is stored verbatim (capped at 2000 characters) so the turns are never silently discarded.
- Docs: `docs/context-manager.md` (pipeline + API), `docs/hooks.md` (hook), `docs/chat-server.md` / `docs/wiki/chat-api.md` (summary fields), `docs/wiki/context-manager.md` (user guide), `docs/wiki/configuration.md`, `docs/config-system.md`, `docs/wiki/what-works-now.md`, `Mimir-Implementation-Context.md` updated.
- Version bumped 0.148.1 → 0.149.0 (minor — new feature subsystem).

## [0.148.1] — 2026-08-25

### Fix: Obsidian export/import review fixes (PR #504)

- `month_number` no longer panics on non-ASCII month tokens (e.g. `éé 2025`): it derives at most the first three characters and returns `None` for unsupported month names.
- Preference import is value-aware: a changed vault value that loses to a user-set or equal/higher-confidence stored preference is reported in the outcome instead of silently skipped (dry-run predicts the conflict), unchanged values stay idempotent with no audit-log churn, and `preferences_updated` is only counted when the upsert actually overwrites.
- A note without a frontmatter `type` or without a `#` heading no longer retypes or renames an existing entity — only explicitly supplied metadata is applied, so hand-written or heading-stripped notes stop silently changing stored entities.
- `render_all` sorts exported documents by relative path (the documented ordering contract) and `render_document` returns a named `RenderedDocument` struct instead of a positional five-element tuple.
- The daemon's `POST /kb/import` runs vault traversal and file reads on a blocking task so a large vault never stalls the async worker.
- `mimir kb export` creates parent directories for nested relative paths before writing each file.
- Docs: the wiki guide describes the export as a human-readable working copy rather than a complete graph backup and states that global preferences remain outside the exported vault and are not restored by import.
- Version bumped 0.148.0 → 0.148.1 (patch — bugfixes and documentation).

## [0.148.0] — 2026-08-25

### Feature: Obsidian export/import — Markdown + YAML frontmatter + wiki-links (issue #62)

- `mimir kb export` renders the whole knowledge graph as Obsidian-compatible Markdown: one `.md` file per entity with YAML frontmatter (`entity_id`, `type`, `aliases`, timestamps), wiki-links (`[[Name]]`) for entity-object facts, and the four-section grammar (`Dates` for event-overlay facts, `Relationships`, `Preferences`, `Facts`). The bundle comes from the daemon's `GET /kb/export` and is written to `--dir`, else `knowledge.export_dir` (env `MIMIR_KNOWLEDGE_EXPORT_DIR`), else `~/AgentKnowledge`; `--stdout` prints the files with `<!-- mimir: {name} -->` separators and `--json` dumps the raw `ExportResponse`.
- `mimir kb import <path>` parses a vault directory (daemon `POST /kb/import`, loopback-gated) back into the graph through the shared `normalize_and_insert` pipeline: `entity_id` anchors re-imports to existing entities (otherwise the canonical name-resolution chain, issue #182), name/type/alias changes are applied, exact existing triples are skipped, imported facts default to `source_type=Import` confidence 0.80 (a `confidence: N` attribute overrides, clamped to `[0, 1]`), event facts recreate the events overlay, and sensitive facts still land in `pending_confirmation`. `--dry-run` plans and reports without writing.
- New shared grammar in `mimir-knowledge/src/obsidian/` (render and parse share one grammar so the two directions cannot drift), `NormalizedFact.confidence` per-fact override, `EventType`/`RecurrenceType`/`PreferenceCategory` wire-name `as_str`/`FromStr` pairs (reused by LLM extraction parsing, DRY), and entity/preference/event query helpers for rendering and existence checks.
- Docs: `docs/obsidian-export-import.md` (format spec + architecture), `docs/wiki/obsidian-export-import.md` (user guide), CLI command docs updated, roadmap 2.16/2.17 and success criteria marked delivered, `Mimir-Implementation-Context.md` updated.
- Version bumped 0.147.2 → 0.148.0 (minor — new feature subsystem).

## [0.147.2] — 2026-08-25

### Fix: Windows-safe TOML paths in the CLI integration-test fixture (PR #503)

- The shared `TestDaemon` fixture now escapes backslashes when interpolating `socket_path`, `context_db`, `kg_db`, and `jobs_db` into the generated `config.toml` template, so Windows temp paths (e.g. `C:\Users\...`) produce valid TOML instead of invalid escapes like `\U`.
- A unit test pins the escaping behaviour (Windows backslashes doubled, Unix paths unchanged).
- Version bumped 0.147.1 → 0.147.2 (patch — test-fixture bugfix).

## [0.147.1] — 2026-08-25

### Fix: Unix socket transport review fixes (PR #503)

- The daemon no longer unlinks a socket pathname before proving it is stale: startup attempts a bounded 500 ms connection first, fails with an "already in use" error when another daemon holds the path, and removes the file only after a confirmed stale-listener failure. Shutdown cleanup is owned by the listener task, and the post-abort cleanup verifies no replacement daemon took the pathname over.
- `mimir-client::build_client` keeps a consistent `unix_socket` parameter on every platform, fixing non-Unix (Windows) builds, and both Unix-socket integration tests in `mimir-server` are gated with `#[cfg(unix)]` so the Windows test build compiles.
- The bounded 500 ms socket liveness probe is shared via `mimir_core::config::socket_is_live` (used by the CLI daemon guard and the daemon's pre-bind stale-socket check) so liveness semantics cannot drift.
- CLI integration coverage: a `TestDaemon` helper runs the CLI without `MIMIR_BASE_URL`, plus an end-to-end test proving `mimir status` reaches the daemon over the Unix socket.
- Docs: transport precedence (`MIMIR_BASE_URL` → Unix socket → TCP) is stated explicitly in `docs/cli.md`, `docs/uds-transport.md`, and the VISION design docs; `docs/daemon-guard.md` and `docs/wiki/daemon-auto-start.md` describe the transport-aware probe; the generated config comment documents the platform-derived `<data_dir>/mimir.sock` default.
- Version bumped 0.147.0 → 0.147.1 (patch — backwards-compatible fixes and documentation).

## [0.147.0] — 2026-08-25

### Feature: Unix domain socket transport for local CLI↔daemon communication (issue #25)

- The daemon now serves the same Axum router on a Unix domain socket alongside the TCP listener. On Unix the socket is enabled by default at `<data_dir>/mimir.sock` (override with `server.socket_path` or `MIMIR_SERVER_SOCKET_PATH`; `~` is expanded), the parent directory is created, a stale socket file left by a crashed daemon is removed before binding, the file is chmod'ed `0600`, and it is removed on graceful shutdown (or aborted shutdown after a fatal TCP error). A socket that cannot be bound fails daemon startup with a descriptive error instead of silently continuing TCP-only.
- The CLI resolves its transport per invocation in `mimir/src/transport.rs`: `MIMIR_BASE_URL` wins (remote daemon), then the Unix socket (env → config → default), then TCP (`server.bind_addr` → `http://127.0.0.1:8080`). Daemon detection over the socket is a 500 ms connection attempt — a local syscall with no HTTP round trip — so a stale socket file left by a crash is detected as down and the daemon guard auto-starts it. Windows is TCP-only.
- The server's loopback guard and `/stop` attribution now use a transport-independent `LocalPeer` connect-info type: Unix peers are always local (filesystem permissions gate access), TCP peers must still be loopback addresses.
- `mimir-client` gained UDS constructors (`new_uds`, `try_new_uds`, `with_token_uds`, `try_new_with_token_uds`) built on reqwest 0.13's native `unix_socket` connector — no `hyperlocal` dependency.
- Config resolution is DRY: the CLI reads the `[server]` section (bind address and socket path) through one shared parser, and the env-over-config precedence for the socket path is unit-tested without environment mutation.
- Tests: socket-path resolution and precedence in `mimir-core`; full-daemon integration tests in `mimir-server` covering `/health` and `/stop` over the socket, socket cleanup on shutdown, and stale-socket recovery; CLI transport precedence and the connect-based daemon probe in `mimir`.
- Docs: new `docs/uds-transport.md` and `docs/wiki/unix-socket-transport.md`; updated `docs/config-system.md`, `docs/cli.md`, `docs/daemon-guard.md`, `docs/chat-server.md`, `docs/api-authentication.md`, `docs/wiki/configuration.md`, `docs/wiki/what-works-now.md`, `Mimir-Implementation-Context.md`, and the Phase 1 / Phase 2 roadmap and VISION design docs.
- Version bumped 0.146.0 → 0.147.0 (minor — new transport feature).

## [0.146.0] — 2026-08-25

### Feature: Outlook wizard supports single-tenant Microsoft app registrations (issue #467)

- The Microsoft account-type question now offers a fourth option — work or school accounts in this organisational directory only — for single-tenant Entra app registrations ("Accounts in this organizational directory only"). `/organizations/` is only valid for multitenant organisational apps, so the wizard collects the tenant ID or domain and builds tenant-specific authorize/token endpoints (`https://login.microsoftonline.com/<tenant>/oauth2/v2.0/authorize` and `/token`).
- The Outlook preset's OAuth client-ID help and the connector docs now describe the tenant-specific authority, and the docs consistently state the registration requirements: the Supported account types must match the picked audience and the loopback redirect URI `http://localhost/callback` must be registered.
- Tests: `wizard_email_outlook_single_tenant_uses_tenant_specific_endpoints` added, and `wizard_email_outlook_work_account_preselects_organizations_endpoints` clarified as the multitenant organisational path.
- Docs: `docs/cli.md`, `docs/email-connector.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, and `docs/wiki/email-connector.md` updated for the single-tenant option and consistent registration guidance.
- Version bumped 0.145.0 → 0.146.0 (minor — new single-tenant wizard option).

## [0.145.0] — 2026-08-25

### Feature: Outlook wizard preset picks the Microsoft login endpoint by account type (issue #467)

- The Outlook / Office 365 email preset previously hardcoded the Microsoft identity platform endpoints to the `/common/` tenant, which only works for app registrations with the "All" supported-account audience — a personal-only (Consumer) or org-only registration failed the authorize request with an opaque `userAudience` error. The wizard now asks which Microsoft account type you connect (personal → `/consumers/`, work or school → `/organizations/`, either → `/common/`), pre-fills the matching authorize/token endpoints, and keeps them editable like every other endpoint prompt.
- The Outlook preset's OAuth client ID help text now states that the registration's "Supported account types" must match the picked audience and that the loopback redirect URI `http://localhost/callback` must be registered.
- Tests: `wizard_email_outlook_personal_account_preselects_consumers_endpoints`, `wizard_email_outlook_work_account_preselects_organizations_endpoints`, the updated oauth-only / common-default paths, and a pinned-prompt test asserting the account-type question is asked and the client-ID guidance states the audience requirement plus the redirect URI. The scripted prompt driver now records prompt messages so guidance text is pin-able.
- Docs: `docs/cli.md`, `docs/email-connector.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, `docs/wiki/email-connector.md`, and `Mimir-Implementation-Context.md` updated; the roadmap checklist in `VISION/09-Roadmap/Phase-3-Connectors.md` notes the account-type-aware endpoints.
- Version bumped 0.144.4 → 0.145.0 (minor — new interactive wizard feature).

## [0.144.4] — 2026-08-24

### Refactor: search_messages builds one query instead of two duplicated SQL blocks (issue #500)

- `ContextManager::search_messages` previously kept two nearly identical `sqlx::query` blocks — one filtering by `m.session_id = ?2` with `LIMIT ?3`, one searching all sessions with `LIMIT ?2` — so every change to the query shape (SELECT list, snippet call, join, ordering, snippet window) had to be applied twice. The query is now assembled once via `sqlx::QueryBuilder`: the shared SELECT/join/order exists in a single place and the session clause and LIMIT are appended conditionally, so the two paths cannot drift.
- A drift-guard integration test (`search_messages_filtered_and_unfiltered_agree`) asserts the session-filtered and unfiltered paths return identical rows and snippets for the same content.
- Docs: `docs/context-manager.md` updated. No behaviour change.
- Version bumped 0.144.3 → 0.144.4 (patch — backwards-compatible refactor).

## [0.144.3] — 2026-08-24

### Fix: FTS5 conversation search matches all terms in any order instead of requiring an exact phrase (issue #493)

- `escape_fts5` wrapped the whole query in double quotes, so every multi-word `search_conversation_history` query became an exact phrase: in the 2026-08-24 Travelodge ask, "check in time" / "checkin" mostly returned `[]` (the phrase never appears verbatim), and the one repeated hit was the `/memory` dump's housing heading "Landlord Inventory and Check-In" — a false positive the model could not distinguish from "no results", so it kept searching for ~100 calls.
- A new `escape_fts5_tokens` helper (`mimir-core/src/fts5.rs`) splits the query into tokens on any run of non-alphanumeric characters — mirroring the FTS5 unicode61 tokenizer, so `check-in` becomes `check` + `in` — and AND-combines the double-quoted tokens, so every term must match in any order while FTS5 operators (`AND`, `OR`, `NOT`, `*`, `-`, parentheses) stay fully neutralised. A query that is itself wrapped in double quotes keeps exact-phrase semantics via the existing `escape_fts5`. `search_messages` now uses the token helper; entity search keeps phrase semantics.
- The snippet window grew from 10 to 30 tokens on each side of the hit, so a match inside a long message surfaces the surrounding answer instead of a bare marker.
- Tests: `escape_fts5_tokens` unit tests (AND joining, hyphen splitting, operator neutralisation, separators, quoted-phrase fallback, unicode, whitespace) and `search_messages` integration tests (terms in any order, AND-not-phrase, `check in` / `check-in` / `checkin` surfacing the hotel context while the housing "Check-In" heading is excluded, and the snippet window surfacing context 25 tokens before the hit). The snippet-window test fails against the old 10-token window.
- Docs: `docs/wiki/conversation-search.md`, `docs/context-manager.md`, `docs/tools-registry.md`, and `docs/benchmarks.md` updated; the `search_conversation_history` tool schema now tells the model that all terms must match in any order.
- Version bumped 0.144.2 → 0.144.3 (patch — backwards-compatible bugfix).

## [0.144.2] — 2026-08-24

### Fix: docs/llm-provider.md hard-wrapped Errors paragraph (issue #483)

- `scripts/tests/md-reflow_test.sh` — the AGENTS.md single-line-prose regression guard from issue #294, enforced per `docs/workspace.md` — failed at HEAD because commit 51a9587 (PR #480 review) appended the "Turn persistence is atomic" sentence to the `## Errors` paragraph of `docs/llm-provider.md` without a blank line, so the paragraph was hard-wrapped across two source lines and `scripts/md-reflow --check` reported the file would reflow.
- The two source lines are joined back into the single flowing paragraph, exactly what `scripts/md-reflow --reflow` emits, with no content change; `scripts/md-reflow --check` and `scripts/tests/md-reflow_test.sh` are green again.
- Version bumped 0.144.1 → 0.144.2 (patch — docs-only bugfix).

## [0.144.1] — 2026-08-24

### Fix: Email IMAP post-login session reads are now bounded (issue #481)

- Every socket read after authentication was unbounded, so a network path that black-holed mid-session (after the #476 connect/handshake bounds) still wedged the runner cycle indefinitely: `EXAMINE`, the `CAPABILITY` probe behind `supports_idle`, the streamed `UID FETCH` response, the `IDLE` init / `DONE` handshakes (the `wait_with_timeout` bound alone does not cover them), and the best-effort `LOGOUT` all awaited socket reads with no timeout.
- A new `read_timeout_secs` config field (default 60) bounds every post-login socket read at the transport boundary with an idle timeout that resets on each byte received, so a slow-but-alive connection — including a large `BODY.PEEK[]` response that takes longer than 60 s in total — is never cut off while a stalled read fails fast. Expired reads surface as `ConnectorError::Network`, with two deliberate exceptions: the `IDLE` `DONE` handshake maps to `IdleResult::ConnectionLost` (the session is gone) and the best-effort `LOGOUT` ignores its error, so the supervisor's exponential backoff / circuit breaker run as designed for every failure the sync operation actually returns.
- The config JSON schema now also advertises the #476 `connect_timeout_secs` / `handshake_timeout_secs` fields, which were missing from the schema.
- Tests: fake-socket tests assert a stalled `EXAMINE`, `CAPABILITY`, `UID FETCH`, `IDLE` init, and `LOGOUT` each fail (or, for the best-effort logout, return) within the read budget as `ConnectorError::Network`, a stalled `IDLE` `DONE` handshake surfaces as `IdleResult::ConnectionLost`, and a chunked `UID FETCH` response with gaps below the budget but a total duration above it still succeeds; the config test pins the default and an override.
- Docs: `docs/email-connector.md`, `docs/wiki/email-connector.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.144.0 → 0.144.1 (patch — backwards-compatible bugfix).

## [0.144.0] — 2026-08-24

### Fix: chat streams report "server error 500: internal server error" on LLM provider failures

- When the upstream LLM provider failed (e.g. ollama.com returning `503` "model temporarily overloaded"), the daemon's retry loop discarded the underlying error and the SSE stream sent a generic `internal server error`, so the CLI showed only "Stream error: server error 500: internal server error" with no actionable detail.
- `LlmError::RetryExhausted` now preserves the last failure (`last_error`), and both the native `/chat/stream` and the OpenAI-compatible `/v1/chat/completions` stream surfaces send the flattened, length-bounded failure message in the terminal `error` SSE frame, so the client reports the real cause (e.g. the overloaded model name) instead of a masked message.
- Tests: the core retry test asserts the preserved last error against a deterministic `503` server, the SSE error-message helper is unit-tested for single-line flattening and length bounding, the chat/OpenAI stream integration tests assert the error frame carries the LLM failure detail, and the OpenAI first-attempt failure test asserts the bounded detail in the pre-SSE `500` body.
- Docs: `docs/llm-client.md`, `docs/chat-server.md`, and `docs/wiki/chat-api.md` updated.
- Version bumped 0.143.1 → 0.144.0 (minor — bugfix plus the `LlmError::RetryExhausted` `last_error` field addition, a breaking change to an internal crate API acceptable per the project's internal-API policy; mirrors the 0.125.0 precedent).

## [0.143.1] — 2026-08-24

### Fix: PR #488 review — per-sub-tool progress completion, SSE test precision

- The retrieval agent now emits each sub-tool's `ToolProgress::Finished` event as soon as that sub-tool completes, instead of waiting for the slowest sub-tool in the round (`join_all` still collects results in input order for the LLM result messages). Streaming clients therefore see research steps complete in real time.
- The streaming chat integration test now parses individual SSE frames and asserts the `tool_call` frame for `kg_query` carries its result, instead of combining independent whole-stream searches.
- Docs: `docs/retrieval-agent.md` clarifies that progress is reported for each non-termination sub-tool call, excluding the private `finish_retrieval` tool.
- Version bumped 0.143.0 → 0.143.1 (patch — backwards-compatible bugfix and documentation update).

## [0.143.0] — 2026-08-24

### Fix: long retrieval-heavy chat requests fail with "error decoding response body" (issue #487)

- The client's default 120s total request timeout was applied to every request, including `/chat/stream`. A query that triggers `retrieve_context` spends ~60s in the retrieval agent and then streams a long final answer, so the wall-clock deadline fired mid-body; reqwest wraps the mid-body timeout as a decode error, so the CLI reported the misleading "Stream error: HTTP error: error decoding response body" while the daemon stayed healthy.
- `POST /chat` now overrides the total timeout per request to 10 minutes (`MimirClient::CHAT_TOTAL_TIMEOUT`), and `POST /chat/stream` to a 30-minute backstop (`MimirClient::CHAT_STREAM_TOTAL_TIMEOUT`) plus a 60-second per-chunk read timeout (`MimirClient::CHAT_STREAM_READ_TIMEOUT`). The daemon already emits SSE keep-alive comments every 10s, so the read timeout only fires when the stream is genuinely wedged; a slow-but-alive stream is never cut off.
- The client SSE parser now accepts `Result<Bytes, ClientError>` input (previously `reqwest::Error`), so the read-timeout error can be surfaced as a `ClientError::Connection` instead of being forced through the reqwest error type.
- Streaming chat now surfaces the retrieval agent's individual sub-tool calls (`kg_query`, `kg_search`, `kg_related`, `search_conversation_history`) as `tool_call_start` / `tool_call` SSE events via a per-request progress channel (`mimir_core::tools::ToolProgress`, `ToolContext::with_progress`, `RetrievalAgent::with_progress`), so the CLI shows the research steps instead of a single "Retrieve Context…" indicator that looks frozen. Blocking paths (`/chat`, `/v1/chat/completions`) pass no channel and run silently.
- Tests: client tests prove a response delayed beyond the default total timeout still streams (blocking and streaming), the read timeout fires on a silent stream and resets on each chunk, the retrieval agent emits Started/Finished progress for sub-tool calls, and the streaming handler forwards them as SSE events end-to-end.
- Docs: `docs/tool-call-visibility.md`, `docs/retrieval-agent.md`, `docs/wiki/tool-calls-in-chat.md`, and `docs/wiki/retrieval-agent.md` updated.
- Version bumped 0.142.3 → 0.143.0 (minor — bugfix plus new public progress API and client timeout constants).

## [0.142.3] — 2026-08-24

### Fix: Email connector IDLE cycles fail on providers that close idle connections (issue #485)

- The Email connector's default IDLE wait (28 min) raced the ~28-minute inactivity close of Microsoft's IMAP service: when no mail arrived during a window, the server dropped the connection just as the client's `DONE` handshake ran, so every no-mail cycle failed with `Connection reset by peer`. A fresh connector's first cycle failed this way before its backfill could be extracted, so the connector stayed authenticated with no errors while never ingesting any mail — the reported Outlook symptom. The default `idle_timeout_secs` is now 1500 (25 min), safely inside both Microsoft's ~28-minute close and RFC 2177's 29-minute re-issue guidance.
- An IDLE window that ends without a push is now always followed by an incremental `UID FETCH` on the same connection, so mail that arrived during the window is never stranded even if the server never pushed a notification (or the notification lost the timeout race).
- A server that drops the IDLE connection mid-window (provider inactivity close or a network drop) is no longer a cycle failure: the cycle reports its progress (the first-sync backfill cursor is persisted and the staged mail is extracted) and marks a re-sync pending, so the next cycle re-fetches the window immediately before re-entering IDLE. The pending re-sync is cleared by the next successful fetch rather than by `on_cycle_succeeded`, so a dropped-IDLE cycle is always followed by a re-fetch.
- A provider that keeps dropping IDLE connections (an inactivity limit shorter than the configured timeout, or a flaky path) can no longer drive an unbounded immediate-reconnect loop: the first two consecutive `ConnectionLost` outcomes still report progress, but the third fails the cycle so the supervisor's exponential backoff applies, and the pending re-sync survives the failure so the post-backoff cycle still re-fetches the window before re-entering IDLE (PR #486 review).
- Tests: the fake IMAP server can now drop the connection during IDLE and append mail without an `EXISTS` push; new tests cover the timeout-fetch, the dropped-IDLE backfill cursor, the dropped-IDLE zero-fetch cycle, and the re-fetch-before-IDLE sequence. All 23 transport tests and the full `mimir-connectors` suite pass.
- Review fixes (PR #486): the dropped-IDLE seeded first-sync cursor (`UIDNEXT − 1`) is now pinned by a test, both IDLE error arms log the underlying `async_imap` error at `debug` level before reporting `ConnectionLost`, and the docs' stale "~30 minutes" provider-close figure is corrected to ~28 minutes.
- Docs: `docs/email-connector.md` and `docs/wiki/email-connector.md` updated (25-minute default, timeout-fetch, dropped-IDLE handling).
- Version bumped 0.142.2 → 0.142.3 (patch — bugfix).

## [0.142.2] — 2026-08-24

### Fix: unused `FunctionCall`/`ToolCall` imports removed from context trim-fallback test (issue #478)

- `trim_fallback_keeps_turn_ending_in_assistant_tool_calls` in `mimir-core/src/context/tests.rs` imported `FunctionCall` and `ToolCall` from `crate::llm::types` while its body constructs the values with fully-qualified paths, so `cargo clippy --workspace --all-targets` emitted an unused-imports warning that fails any `-D warnings` gate. The two unused imports are dropped; the test body is unchanged and all 35 context-manager tests still pass.
- Version bumped 0.142.1 → 0.142.2 (patch — build hygiene).

## [0.142.1] — 2026-08-24

### Fix: bounded IMAP connect / TLS-handshake / greeting timeouts (issue #476)

- The Email IMAP transport now applies configurable network budgets to every step of the connection path: `connect_timeout_secs` (default 10) bounds the TCP connect and `handshake_timeout_secs` (default 30) bounds the rustls handshake, the first server response (the IMAP greeting), and the `LOGIN` / `AUTHENTICATE` response as one shared deadline, so a black-holed network path fails the cycle fast as `ConnectorError::Network` and the supervisor backoff / circuit breaker run as designed instead of wedging the runner indefinitely. Existing stored configs load unchanged (serde defaults).
- Tests: a never-resolving TCP connect, a real rustls handshake against a local listener that accepts but never speaks TLS, a greeting that never arrives, a `LOGIN` that is never answered, and a staged greeting delay followed by a stalled login each fail within the shared budget; config parse tests pin the defaults and explicit overrides. The CalDAV, OAuth-refresh, and geocoder HTTP clients were audited in the same pass and already carry reqwest-level timeouts.
- Docs: `docs/email-connector.md`, `docs/wiki/email-connector.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.142.0 → 0.142.1 (patch — bugfix).

## [0.142.0] — 2026-08-24

### Fix: OpenAI-compatible endpoint no longer treats missing `user` as incognito (issue #473)

- `POST /v1/chat/completions` removes the implicit-incognito path entirely: a request without a `user` field (or with a blank one) now keys the fixed `default` session instead of silently skipping persistence and learning. Every `/v1` request resolves or creates a session, persists the user message and assistant response, exports write-capable tools, and fires the `remember.chat` hook on completion — a generic phone LLM app that cannot send the `user` field now feeds the same memory as every other client.
- The OpenAI `user` field remains a conversation key only: a fixed value resumes that session exactly as before, and a blank value is treated as absent (no session keyed on `""`). An explicit incognito option on the OpenAI surface is deliberately not added; incognito stays on the native route (`incognito: true`) and the CLI (`mimir chat --incognito`).
- The route's dead incognito machinery was removed: the `INCOGNITO_COUNTER` branch, the ephemeral trailing-segment conversation assembly (`convert_message`, `last_user_index`), the per-request `incognito` flag, and the write-tool suppression now never apply on `/v1`.
- Tests: unkeyed blocking and streaming requests persist the default session, resume it across requests (blank `user` included), and dispatch `remember.chat` end-to-end (the fact lands in the knowledge graph); the shared learning fixtures moved from `chat_learning_tests.rs` into `mimir-server/tests/common/mod.rs` (DRY with the native chat suites).
- Review fixes (PR #480): the per-session permit is acquired before the rollback baseline is read so a failed request can never delete another request's persisted messages; a client disconnect before stream completion rolls the turn back instead of leaving an orphaned user/tool message in the session; and tool-message persistence failures now fail the turn atomically (rollback + SSE `error` in streaming) instead of returning a response whose tool-derived output was never stored.
- Docs: `docs/chat-server.md`, `docs/llm-provider.md`, `docs/wiki/llm-provider.md`, `VISION/08-Architecture/Multi-Device.md`, `Mimir-Implementation-Context.md`, and the `OpenAiChatRequest.user` doc comment updated (default session key, no incognito path on `/v1`).
- Version bumped 0.141.5 → 0.142.0 (minor — deliberate behaviour change on the public OpenAI-compatible surface).

## [0.141.5] — 2026-08-24

### Fix: connector sync accepts unprobed auto-mode connectors until the mode resolves (issue #475)

- The manual-sync push gate now consults the *resolved* mode (`Connector::mode_if_resolved`) instead of the optimistic `mode()`: an `auto`-mode email connector whose IMAP `IDLE` capability probe has not completed yet (`mode_if_resolved()` is `None`) accepts `mimir connector sync` as the force-retry, matching the `-` the list already shows. Only a connector whose mode is *proven* push keeps rejecting manual sync with `CONNECTOR_PUSH_UNSUPPORTED`; config-pinned `poll` / `idle` modes resolve deterministically and behave as before.
- The runner's push-success loop now drains the trigger channel: a trigger accepted while the mode was unprobed is never stranded if the capability probe resolves to push after the gate check but before the runner reads the channel (the awaiting `sync` caller would otherwise hang forever).
- The `MockConnector` gains a runtime mode-resolution override (`with_mode_resolution_override`) mirroring the existing `with_mode_override`, so supervisor tests can flip a connector between unprobed, resolved-push, and resolved-polling states; the new control test covers all three transitions.
- Docs: `docs/cli.md`, `docs/connectors-framework.md`, `docs/connector-management.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, and `docs/wiki/what-works-now.md` updated (manual sync is rejected only once the mode has resolved to push).
- Version bumped 0.141.4 → 0.141.5 (patch — bugfix).

## [0.141.4] — 2026-08-24

### Fix: PR #477 review — source-compatible pool enqueue APIs and atomic stream admission

- `LlmWorkerPool` regains the original public `enqueue_chat_message`, `enqueue_chat`, `enqueue_chat_stream`, `enqueue_system_chat_message`, `enqueue_system_chat`, and `enqueue_system_chat_stream` signatures (no `LlmRequestOverrides` argument); the override-taking behaviour moves to explicit `*_with_overrides` variants, so callers compiled against 0.141.2 keep compiling after the upgrade.
- The OpenAI-compatible streaming route (`/v1/chat/completions` with `stream: true`) now admits the first stream job before the SSE response starts, so a full user queue returns `503` + `Retry-After: 5` instead of surfacing an SSE error event after `200 OK`.
- Mock HTTP servers in the pool/client tests read the complete request (headers + `Content-Length` body) before parsing JSON instead of trusting a single `TcpStream::peek`; `MockLlmClientBuilder::push_stream_error` queues immediate stream admission failures for regression tests.
- Version bumped 0.141.3 → 0.141.4 (patch — bugfix).

## [0.141.3] — 2026-08-23

### Fix: chat requests no longer bypass the LLM worker pool when overrides are applied (issue #465)

- `LlmBackend::with_model_override`, `with_temperature_override`, and `with_max_tokens_override` clones now keep the worker pool: the override is recorded on the client and carried through the job payload (`LlmRequestOverrides`) instead of swapping the pool for a direct HTTP client. Interactive chat (`/chat`, `/chat/stream`) and the OpenAI-compatible provider surface (`/v1/chat/completions`) therefore enqueue on the pool's user queue, so `LlmError::QueueFull` → `503` + `Retry-After: 5` backpressure is live on the hot path and the status endpoint's `queue_depth_user` reflects real chat load.
- The pool worker applies job overrides when building the upstream request (model, temperature, `max_tokens`) without rebuilding its HTTP client; `LlmBackend::max_tokens` reports the effective override value, and `fetch_model_context_window` probes the effective model.
- Tests: override clones preserve pooling; pooled temperature overrides reach the upstream request; pool jobs apply model/temperature/`max_tokens` overrides end-to-end.
- Docs: `docs/llm-worker-pool.md`, `docs/llm-provider.md`, `docs/chat-server.md`, and `docs/wiki/llm-client.md` updated (pooled overrides, live 503 backpressure).
- Version bumped 0.141.2 → 0.141.3 (patch — bugfix).

## [0.141.2] — 2026-08-23

### Fix: OAuth PKCE redirect URI uses `localhost` so Microsoft Entra registrations match

- The interactive PKCE login (`mimir connector add` / `auth` with `auth.kind=oauth`) now sends `http://localhost:<port>/callback` as the redirect URI instead of `http://127.0.0.1:<port>/callback`. Microsoft Entra ignores the port component only for `localhost` redirect URIs — an IP-literal loopback URI with a random ephemeral port can never match a registration, so the Outlook / Office 365 login failed with `invalid_request: redirect_uri is not valid`. The callback listener still binds `127.0.0.1` only; browsers resolve `localhost` and fall back to IPv4. Google desktop-app clients are unaffected (they accept any loopback port on `localhost` or `127.0.0.1`).
- Microsoft app registrations must register the loopback redirect `http://localhost/callback` (any port matches); the wizard's Outlook preset help text now states the exact URI to register.
- Tests: the mock-OAuth PKCE E2E assertion now expects the `localhost` redirect URI; the full PKCE round trip and the daemon-level CLI OAuth E2E pass.
- Docs: `docs/oauth-client.md`, `docs/wiki/connectors.md`, `docs/cli.md`, and `docs/wiki/cli-commands.md` updated (redirect URI host, provider registration requirements).
- Version bumped 0.141.1 → 0.141.2 (patch — bugfix).

## [0.141.1] — 2026-08-23

### Fix: OpenAI provider surface review hardening (PR #466 review)

- Tool validation: client-supplied `tools` are validated before any session creation or persistence — a tool must be a `function` tool with a non-empty name and, when present, an object `parameters` schema; malformed definitions return `400 invalid_request_error` with `param: "tools"` instead of an opaque upstream failure.
- Orphaned-turn prevention: a failed turn (queue-full, LLM error, or mid-stream failure) now rolls back the messages the request persisted (via the new `ContextManager::max_message_id` / `delete_messages_after` helpers), so the session never keeps a user-only final turn. Tool definitions are validated before session creation, so rejected requests leave nothing behind.
- Streaming: tool-call deltas are buffered per round and only client-tool deltas are emitted, in `index` order — internal Mimir tool calls (and their accumulated `index` values) never reach the client stream. A mid-stream failure now emits an `event: error` frame followed by `data: [DONE]` so clients can distinguish failed streams from completed ones. Usage is accumulated across tool rounds in both the blocking and streaming paths (`LlmBackend::max_tokens` typed accessor added; `ToolCall.index` is no longer serialised upstream).
- Trimming: `tool`-role messages are excluded from the unknown-token-count probe (they never carry token counts), so tool-using sessions keep precise token trimming; a turn whose final row is an assistant tool-call message is treated as in-flight in both trimming paths and is never deleted before the client sends its tool results.
- Preset probing: `PersonalityCache::has_preset` checks preset existence without emitting per-request diagnostics, so upstream model names no longer log unknown-preset warnings on every request. The resumed keyed session's first-writer-wins system prompt is now explicitly documented.
- Wire shape: tool-call responses always serialise `content` (explicit `null`), matching the OpenAI response shape.
- Docs: `docs/context-manager.md`, `docs/wiki/context-manager.md`, `docs/llm-provider.md`, `docs/wiki/llm-provider.md`, `docs/chat-server.md`, `docs/wiki/chat-api.md`, `docs/wiki/what-works-now.md`, and `VISION/08-Architecture/Multi-Device.md` updated (schema columns, tool persistence, incognito persistence boundary, streaming failure framing, TLS claims limited to reverse-proxy traffic).
- Tests: concurrency test now runs on a multi-thread runtime; new regression tests for tool-aware trimming, tool validation, usage accumulation, multi-tool stream ordering, stream-error framing + rollback, server-tool delta suppression, and queue-full rollback.
- Version bumped 0.141.0 → 0.141.1 (patch — bugfixes).

## [0.141.0] — 2026-08-23

### Feature: OpenAI-compatible provider surface — /v1/models + /v1/chat/completions (issue #388)

- The daemon now exposes an OpenAI-compatible provider surface so any app or device that speaks the OpenAI chat-completions API can use Mimir as its LLM provider: `GET /v1/models` lists personality presets as models (with descriptions), and `POST /v1/chat/completions` answers blocking and streaming requests mapped onto Mimir's session, personality, and worker-pool infrastructure.
- Session mapping: Mimir is single-tenant, so the OpenAI `user` field is a conversation key backed by a new nullable unique `user_key` column on the sessions table (with migration) — a fixed `user` resumes one persistent session in the central profile, race-safe via a partial unique index. The client's `messages` array is a stateless echo: only the last user message starts a new turn, and trailing `tool` messages continue an in-flight turn. Requests without `user` are incognito-style (memory context injected, nothing persisted, no learning hooks).
- Model mapping: `model` names matching a personality preset select that preset; unknown names pass through as upstream model overrides with the configured default personality.
- Tools: client-supplied `tools` schemas merge with Mimir's server-side tools (server tools always available; on a name collision the server-side tool wins and the client's definition is silently dropped). Client tool calls are returned to the client (`finish_reason: "tool_calls"`), the assistant tool-call message and server tool results are persisted, and the client's follow-up `tool` messages continue the turn. `remember` stays a server-side hook and fires only when the turn completes.
- Sampling: per-request `temperature` wins over config; per-request `max_tokens`/`max_completion_tokens` applies only when the client sends it (no default cap), via the new `LlmBackend::with_max_tokens_override`.
- Streaming: OpenAI chunk framing (`chat.completion.chunk`, `delta.role` on the first chunk, `finish_reason`), `stream_options.include_usage` final usage chunk, and terminal `data: [DONE]`. Internal tool activity stays invisible on v1 (tracked in #464).
- Errors: `/v1` routes return the OpenAI error JSON shape; a full worker-pool queue maps to `503` with `Retry-After: 5` (defensive until the pool bypass in #465 is fixed).
- ContextManager: tool-message persistence (`add_tool_message`, `add_assistant_tool_calls_message`) with `tool_calls`/`tool_call_id` columns (with migration) and export round-trip, plus turn-based trimming so tool messages are never orphaned when old turns are trimmed.
- Tests: OpenAI wire-type round-trips, session-mapping/migration/tool-persistence/trim unit tests, `max_tokens` override tests, and 16 server integration tests covering model listing, blocking/streaming shapes, session resumption, incognito, preset selection, client-tool round-trips, server-tool collision/execution, auth, and the 503 error shape.
- Docs: new `docs/llm-provider.md` (technical) and `docs/wiki/llm-provider.md` (usage); `docs/chat-server.md`, `docs/wiki/server.md`, `docs/wiki/chat-api.md`, `docs/wiki/what-works-now.md`, `README.md`, `Mimir-Implementation-Context.md`, and `VISION/08-Architecture/Multi-Device.md` updated.
- Version bumped 0.140.1 → 0.141.0 (minor — new feature).

## [0.140.1] — 2026-08-23

### Fix: calendar wizard rejects an OAuth scope list that parses to empty (issue #462)

- The Custom CalDAV OAuth path (`calendar_oauth_questions` in `mimir/src/connector/wizard.rs`) now mirrors the email OAuth guard: a non-blank scope answer that parses to zero scopes (e.g. `", ,"`) is rejected with "OAuth scopes is required" before any auth config is built, so the wizard can never produce an authorize request with `"scopes": []` and fail the PKCE flow. A blank answer still keeps the Google Calendar default scope, so the Google preset's prompt behaviour is unchanged.
- Tests: new scripted-prompt wizard test `wizard_caldav_oauth_rejects_parsed_empty_scopes` covering the Custom CalDAV OAuth flow with a `", ,"` answer (connector suite now 70 tests).
- Docs: `docs/connector-management.md`, `docs/unit-tests.md`, and `docs/wiki/connectors.md` updated to describe the guard.
- Version bumped 0.140.0 → 0.140.1 (patch — bugfix).

## [0.140.0] — 2026-08-23

### Feature: wizard email + calendar provider presets, generic Email connector type (issue #400)

- The interactive wizard (`mimir connector add`) now offers email provider presets that pre-fill the IMAP defaults and provider guidance: Gmail (`imap.gmail.com:993`, Google OAuth endpoints + `https://mail.google.com/` scope pre-filled, OAuth first with app-password fallback), Outlook / Office 365 (`outlook.office365.com:993`, Microsoft identity platform endpoints + `https://outlook.office.com/IMAP.AccessAsUser.All offline_access` scope, OAuth 2.0 only — Microsoft retired app passwords for Outlook.com / Exchange Online IMAP), Yahoo (`imap.mail.yahoo.com:993`, app password), Proton Mail Bridge (`127.0.0.1:1143`, app password), iCloud (`imap.mail.me.com:993`, app password), and custom IMAP (free-form, app password or user-supplied OAuth endpoints, with an empty OAuth scope list rejected). Presets are wizard-side defaults only — the backend stays `imap` for every provider, and the sync-mode / first-sync-backfill questions (issue #397) apply to every preset.
- The calendar wizard got the same treatment: Google Calendar (primary-calendar CalDAV collection URL computed from the account email, Google OAuth), iCloud and Yahoo (server URL defaults, app password), and Custom CalDAV. Outlook / Office 365 is deliberately absent — Microsoft exposes no public CalDAV endpoint (a Microsoft Graph calendar backend is deferred as a follow-on).
- The IMAP mail connector type is now the generic `Email` type: `ConnectorType::Gmail` (wire string `gmail`, DB id 1) is renamed `ConnectorType::Email` (wire string `email`, DB id unchanged — migration `054_rename_email_connector_type.sql` renames the seeded `connector_types` row), the legacy `gmail` wire string stays accepted as an input alias (CLI flag form normalizes it to `email`; the daemon's `FromStr` accepts both), the default email slug/display name became `email`/`Email`, and the `gmail` cargo feature was renamed `email` across `mimir-connectors` / `mimir-server` / the no-default-features test matrix.
- Tests: 29 wizard tests covering every email and calendar preset (endpoint/scope/IMAP defaults, auth ordering — Outlook is OAuth-only and custom IMAP rejects blank or parsed-empty scope lists — sync questions), legacy-alias registration, enum wire-contract and discriminant-stability tests, migration seed test, and updated CLI/e2e/server/migrations suites.
- Docs: `docs/email-connector.md`, `docs/connectors-framework.md`, `docs/connector-management.md`, `docs/cli.md`, `docs/oauth-client.md`, `docs/workspace.md`, `docs/unit-tests.md`, `docs/e2e-testing.md`, `docs/fact-extraction-pipeline.md`, `docs/Confidence-Model.md`, `docs/mock-connector.md`, the matching `docs/wiki/` pages, `README.md`, `Mimir-Implementation-Context.md`, and the Phase 3 VISION docs updated.
- Version bumped 0.139.0 → 0.140.0 (minor — new feature; the `gmail` wire alias keeps pre-rename scripts working).

## [0.139.0] — 2026-08-23

### Feature: email facts are contextualised by the message envelope — dates, sender, recipients, spam signals (issue #398)

- The extraction cascade now derives one `EmailEnvelope` per message — sent date (RFC 5322 `Date`), received date (IMAP `INTERNALDATE`), `From`/`To`/`Cc`/`Reply-To`, subject, `List-Unsubscribe` state, and the deterministic spam / forwarded / wrong-recipient signals — and gates the whole cascade on it: the bulk-mail filter (`List-Unsubscribe` or a pure-marketing sender domain) now runs before the iMIP and JSON-LD layers too, so a marketing broadcast carrying a calendar invite or machine-readable receipt can no longer author facts.
- The prose (LLM) layer is anchored with the full envelope plus the current UTC date, so relative phrases ("tomorrow", "next week", "overdue") resolve against real timestamps instead of model guesses; the temporal binding itself stays in Rust.
- Prose facts without an explicit `valid_from` are anchored at the email's sent date (received date as fallback), and an actionable fact (task / deadline / reminder) without an explicit `valid_until` expires 30 days after that anchor (recurring facts are exempt — their recurrence owns the lifecycle) — urgency decays with the email's age, so a two-year-old "pay rent" reminder is recorded as history and can never surface as a current action item. Explicit machine-readable timestamps (iMIP `DTSTART`, JSON-LD reservation dates) are never overwritten.
- Forwarded mail (a `Fwd:`/`FW:` subject prefix or the standard "Forwarded message" body separator) and misdirected mail (the mailbox address appears in neither `To` nor `Cc`) are mined for real facts but never for tasks: `requires_user_action` is forced false and task / deadline / reminder event types are cleared, so they cannot author obligations or action items for the mailbox owner.
- The `connector_item.remember` hook payload and the durable queue-overflow retry entry now carry the `INTERNALDATE` (and the mailbox address), so a re-staged or hook-delayed message reconstructs the same envelope context after a restart.
- Tests: envelope derivation/classification/binding unit tests in `email/envelope.rs`; prompt-content, past-window, and never-actionable prose tests in `email/llm_tests.rs` (including an end-to-end hook-engine assertion against the persisted fact); cascade spam-gate tests for iMIP/JSON-LD in `email/extract_tests.rs`; and an `INTERNALDATE` round-trip test in `email/llm/retry.rs`.
- Docs: `docs/email-connector.md` (new "Envelope context (issue #398)" section), `docs/wiki/email-connector.md` ("Emails are read in context"), `docs/wiki/what-works-now.md`, and `README.md` updated.
- Version bumped 0.138.0 → 0.139.0 (minor — new behaviour, no public API break; `EmailExtractionPayload` gained internal fields).

## [0.138.0] — 2026-08-23

### Fix: chat route no longer re-reads personality presets on every request (issue #453)

- The daemon now owns a `PersonalityCache` in `AppState`; chat requests resolve the active preset through it, and the personalities directory is only re-scanned when a cheap metadata fingerprint (directory mtime + per-file name/size/mtime/kind, never file contents) says preset files changed. Per-request `personality_preset` overrides (`mimir ask -p`, `/personality`) still resolve against the cached registry.
- Invalidation covers edited, added, and removed preset files plus the directory being created after startup; an unreadable directory always rescans so transient errors cannot pin a stale cache. Scan diagnostics are now logged once per scan instead of once per request, while per-request fallback diagnostics (unknown preset names) still log per request.
- Custom preset files above 1 MiB (the same cap as skill files) still load but now emit a scan-time size-advisory warning (`MAX_PRESET_FILE_SIZE` in `mimir-core/src/personality.rs`).
- The one-shot paths (`Personality::new`, `mimir personality list`) are unchanged; `Personality` construction internals were factored into a shared `from_scan` helper (DRY) with the cache.
- Tests: eight new `PersonalityCache` unit tests in `mimir-core/src/personality.rs` cover first scan, cache hits (via `scan_count()`), content/add/remove invalidation, directory creation, warning refresh, and the size advisory.
- Docs: `docs/personality-system.md` and `docs/wiki/personality.md` updated with the caching design and user-facing behaviour.
- Version bumped 0.137.0 → 0.138.0 (minor — new public API `PersonalityCache`).

## [0.137.0] — 2026-08-23

### Feature: opt-in OS-keychain SecretStore backend (issue #188 / F11)

- `KeyringSecretStore` implements the `SecretStore` trait over the `keyring` crate — macOS Keychain, Linux/FreeBSD/OpenBSD Secret Service (gnome-keyring / KWallet over D-Bus), and Windows Credential Manager. Each connector slug is one OS entry (keyring service `mimir`, account = slug) holding the serialized `SecretBundle` JSON — both backends round-trip the same JSON schema losslessly, differing only in formatting (the file store writes pretty-printed JSON, the keyring store compact JSON).
- Feature-gated behind `secrets-keyring` (off by default — headless systemd boxes often lack a Secret Service daemon) and selected at daemon startup with the new `[secrets]` config section: `secrets.backend = "file"` (default) or `"keychain"`, with `MIMIR_SECRETS_BACKEND` env override. Requesting `keychain` in a build without the feature fails daemon startup with an actionable error instead of silently falling back to plaintext files.
- Missing entries follow the `SecretStore` contract (`load` → `Ok(None)`, `delete` → `Ok(())`); platform failures surface as a new feature-gated `SecretError::Keyring` (HTTP 500 via the existing secret-error mapping); slug validation is shared with the file store (`[A-Za-z0-9_-]{1,128}`) before any keyring call.
- Dependency reconciliation: `keyring` is pinned to 3.6.3 because every 4.x release requires Rust 1.88, above the workspace MSRV 1.85 (same cap as `icalendar`, #239). Linux uses the pure-Rust zbus Secret Service stack (`async-secret-service` + `crypto-rust`); keyring's `tokio` feature is deliberately off because zbus's `block_on` panics inside a tokio runtime, and the `sync-secret-service` variant is avoided because it needs the libdbus C library.
- Review fixes: `keyring`'s `Entry` API is blocking even with `async-secret-service`, so every operation is dispatched through `tokio::task::spawn_blocking` onto a dedicated blocking worker (per keyring's own guidance) instead of running inline on the Tokio executor; a worker panic or runtime shutdown surfaces as the new feature-gated `SecretError::KeyringTask`; docs now state the keyring feature's supported targets (Linux, FreeBSD, OpenBSD, macOS, Windows) and make the file-store wording conditional on `secrets.backend = "file"`.
- Docs: `docs/connector-secret-store.md`, `docs/connectors-framework.md`, `docs/config-system.md`, `docs/wiki/connectors.md`, `docs/wiki/configuration.md`, `docs/wiki/what-works-now.md`, `README.md`, `Mimir-Implementation-Context.md`, and both Phase 3 roadmap files updated; the `VISION/09-Roadmap/Phase-3-Plan.md` dependency ledger records the MSRV reconciliation.
- Version bumped 0.136.0 → 0.137.0 (minor — new feature).

## [0.136.0] — 2026-08-22

### Feature: `mimir connector add` just works — wizard auto-activation, sync-mode + backfill choices, resolved mode (issue #397)

- The interactive wizard (`mimir connector add`) now asks the decisions that matter for the Gmail IMAP profile: sync mode ("Continuously — push (recommended)" → `mode: auto`/IDLE vs "Every N minutes — polling" with 5/15/30/60-minute presets or a custom interval → `mode: poll` + `poll_interval_secs`) and whether the first sync imports the existing mailbox (`initial_backfill`, default `true`) or starts from "now" (the cursor is seeded to the mailbox's `UIDNEXT − 1` so existing mail is never fetched).
- After credential ingest the wizard auto-activates the connector (`POST /connectors/{id}/resume`) and syncing starts immediately — an immediate cycle for polling, the backfilled first cycle for push — and the summary prints the active state plus the resolved mode. The flag form keeps the explicit `Setup` → `resume` → `sync` lifecycle for scripts.
- `ConnectorResponse` gains the resolved `mode` (`push` / `polling`), derived per row from the persisted config with no side effects (`ConnectorSupervisor::resolved_mode` + `ConnectorMode::wire_name`); the add summary, `mimir connector list` (new column), and `mimir connector status` (new detail line) use it.
- Push backfill: a fresh push-mode Email connector now fetches the existing mailbox contents before blocking on IDLE, so `resume` on a brand-new connector imports the current inbox instead of waiting for the first new message; mail arriving during IDLE is fetched incrementally from the backfilled UID, and the backfill is deduped against the staged buffer (fake-IMAP-server tests cover both paths).
- `mimir connector sync` on a push connector now returns a 409 that explains push connectors sync automatically (IMAP IDLE / a file watcher) instead of only rejecting; polling-mode connectors keep manual sync.
- PR #457 review fixes: the `mode` of an `auto`-mode Email connector is omitted (never guessed as `push`) until its first capability probe has run and persisted the IMAP `IDLE` capability in the connector's durable state; the "only new content" seed now applies only to a true first sync (a UIDVALIDITY reset still full re-syncs, and a first sync without `UIDNEXT` fails instead of silently full-fetching); manual sync consults the live connector mode, so an `auto` connector that probes as polling keeps `mimir connector sync`; the flag-form example documents `mode=poll` for scripted manual sync.
- Docs: `docs/connector-management.md`, `docs/email-connector.md`, `docs/cli.md`, `docs/wiki/connectors.md`, `docs/wiki/email-connector.md`, `README.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.135.1 → 0.136.0 (minor — new feature).

## [0.135.1] — 2026-08-22

### Fix: PR #456 review feedback — heatmap consistency, reset warning, roadmap cleanup (issue #69)

- The connector confidence band is now labelled `connector (0.7-1.0)` everywhere (SQL, API contract tests, CLI fixture, docs, changelog) so the displayed interval matches the half-open `[0.7, 1.0)` predicate instead of mislabelling values above `0.9`.
- `KnowledgeGraph::heatmap()` now reads every aggregate inside a single read transaction, so a concurrent write cannot interleave between statements and mix graph states.
- `mimir kb reset` now labels the heatmap fact count as non-trashed and states that trashed facts are also deleted permanently, matching the full-wipe scope.
- The reset-flow wiremock test now asserts `"archive": false` in the forget request, rejecting archival behaviour.
- `docs/wiki/kb-heatmap-reset.md` describes the temporal bucket (`valid_from`, falling back to `created_at`) and the confidence distribution as numeric ranges; the delivered `kb heatmap`/`kb reset` rows were removed from the deferred/out-of-scope roadmap sections.
- Version bumped 0.135.0 → 0.135.1 (patch — review fixes).

## [0.135.0] — 2026-08-22

### Feature: kb heatmap + kb reset (issue #69)

- New `mimir kb heatmap [--json]` renders a knowledge-density snapshot of the graph as terminal bar charts: live totals (facts, entities, average confidence), top 10 entities and predicates by fact count (ties by name), facts per `YYYY-MM` month (from `valid_from`, falling back to `created_at`), and the confidence distribution in fixed bands (`explicit (1.0)`, `connector (0.7-1.0)`, `inference (0.4-0.7)`, `casual (<0.4)`). Trashed (forgotten) facts are excluded from every aggregate. Backed by a new read-only daemon route `GET /kb/heatmap` (`mimir-knowledge` `queries/heatmap.rs` + `KnowledgeGraph::heatmap()` facade, `mimir-api-types::HeatmapResponse`, `MimirClient::kb_heatmap`) — no new dependencies, no TUI (the `ratatui` option from the issue was judged unjustified; `--json` is the stable scripting surface).
- New `mimir kb reset` is a dedicated, safer full-wipe flow over the existing `kb forget --all` machinery: it prints live entity/fact counts, requires the exact phrase `DELETE EVERYTHING` (case-sensitive, interactive; the daemon re-validates it), runs a 5-second countdown, then hard-deletes the graph after the daemon creates a timestamped backup (`~/.local/share/mimir/backups/`). Non-interactive scripts keep using `mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"`.
- Tests: knowledge-layer aggregation tests (`mimir-knowledge/tests/heatmap_tests.rs`), daemon route tests (`mimir-server/tests/kb_heatmap_tests.rs`), CLI rendering + reset-flow tests against a wiremock daemon (`mimir/src/kb/tests.rs`).
- Docs: new `docs/kb-heatmap-reset.md` (technical) and `docs/wiki/kb-heatmap-reset.md` (user-facing); `docs/cli.md`, `docs/wiki/cli-commands.md`, `docs/fact-management.md`, `README.md`, `docs/wiki/what-works-now.md`, and the Phase 2/3 roadmap docs updated.
- Version bumped 0.134.1 → 0.135.0 (minor — new feature).

## [0.134.1] — 2026-08-22

### Fix: PR #455 review feedback — contact email trimming and geocoder doc clarifications (issue #227)

- `MIMIR_GEOCODER_CONTACT_EMAIL` now trims surrounding whitespace before being stored, and an all-whitespace value still clears the contact email.
- `docs/config-system.md` fixes the disabled-geocoder sentence (comma before the second independent clause), and `docs/wiki/configuration.md` documents that changing `enabled`, `endpoint`, or `contact_email` requires a process restart because the geocoder is constructed once at startup and is not hot-reloaded.
- Version bumped 0.134.0 → 0.134.1 (patch — review and documentation fixes).

## [0.134.0] — 2026-08-22

### Feature: geocoder configuration — disable toggle, self-hosted endpoint, contact email (issue #227)

- The shared geocoder now honours a `[geocoder]` section in `config.toml` (and `MIMIR_GEOCODER_ENABLED` / `MIMIR_GEOCODER_ENDPOINT` / `MIMIR_GEOCODER_CONTACT_EMAIL` env overrides): `enabled = false` disables geocoding entirely — location facts persist with whatever the producer supplied and the missing coords/place half is never filled in — `endpoint` points at a self-hosted Nominatim instance (the usage policy recommends this for heavy use), and `contact_email` is appended to the policy-compliant `User-Agent`.
- `init_knowledge_graph` now builds `NominatimConfig` from the config section via `impl From<&GeocoderConfig> for NominatimConfig` (`mimir-connectors`) and skips injection when `enabled = false`, replacing the unconditional `with_defaults()` wiring; `DEFAULT_NOMINATIM_ENDPOINT` moved to `mimir-core::geocoder` so the compiled-in config default and the backend share one source of truth.
- Docs updated: `docs/geocoder.md`, `docs/config-system.md`, `docs/entity-locations.md`, `docs/wiki/geocoding.md`, `docs/wiki/configuration.md`, `docs/wiki/what-works-now.md`, `Mimir-Implementation-Context.md`.
- Version bumped 0.133.0 → 0.134.0 (minor — new feature).

## [0.133.0] — 2026-08-22

### Feature: first-class personality preset discovery (issue #387)

- New `mimir personality list` CLI command renders every preset (built-in + custom) as a table with `NAME`, `SOURCE`, and `DESCRIPTION` columns, sorted by name; it runs locally against the config directory and needs no daemon.
- Custom preset files may carry an optional `description` in minimal YAML frontmatter delimited by standalone `---` lines; only the `description` key is supported, unknown keys (e.g. stale tone knobs) warn and are ignored, multi-line descriptions collapse to one line, and files without frontmatter behave exactly as before.
- Diagnostics are no longer silent: an unknown configured preset and malformed, unreadable, or invalid-UTF-8 custom preset files produce warnings — daemon log or `mimir personality list` stderr — while still falling back to `transparent` and exiting successfully.
- `Personality::list_presets` now returns `Vec<PresetInfo>` (name, `PresetSource` Builtin/Custom, optional description; serde-serializable for the future `/v1/models` surface, issue #388), built-in presets ship descriptions, and diagnostics are exposed via `Personality::warnings`.
- The `---`-fenced YAML frontmatter splitter is extracted into `mimir-core/src/frontmatter.rs` and shared with the skills loader (DRY), with exact byte offsets for LF and CRLF line endings.
- Docs updated: `docs/personality-system.md`, `docs/wiki/personality.md`, `docs/cli.md`, `docs/wiki/cli-commands.md`, `README.md`, `docs/wiki/what-works-now.md`, `Mimir-Implementation-Context.md`, and `VISION/01-Core-Agent/Personality.md` (which also drops the stale `remember`-tool wording from the operating-directives section).
- Code-review fixes: a failed personalities-directory resolution is logged exactly once (the stored warning feeds both the daemon log and `mimir personality list` stderr), and `.personality.md` files that would register an empty preset name are ignored like other non-matching files.
- Version bumped 0.132.4 → 0.133.0 (minor — new feature).

## [0.132.4] — 2026-08-22

### Fix: PR #452 review feedback hardens the future-incompat guard and fixes vendored docs (issue #446)

- `scripts/tests/future-incompat_test.sh` now builds in a fresh target directory and inspects `cargo report future-incompatibilities` as well as the clippy output, so a warm `target/` cannot hide dependency future-incompat warnings once `[patch.crates-io]` is dropped; the `SCRIPT_DIR` variable is also renamed to `REPO_ROOT` to match its value.
- `scripts/md-reflow` now skips only the repository-root `vendor/` tree instead of any directory named `vendor` at any depth, matching the scope documented in its README and test, and the doc comment describes the narrower rule.
- The vendored `proc-macro-error2` README diagnostic screenshots gain alt text, and the rustdoc references for `emit_call_site_warning!`/`emit_call_site_error!` in `src/lib.rs` now point at their matching macro pages instead of being swapped.
- Version bumped 0.132.3 → 0.132.4 (patch — build-guard and doc review fixes).

## [0.132.3] — 2026-08-22

### Fix: vendored `proc-macro-error2` removes the dependency future-incompat build warning (issue #446)

- `cargo clippy --workspace --all-targets` no longer warns that `proc-macro-error2 v2.0.1` contains code a future Rust toolchain will reject. The crate is abandoned upstream (last crates.io release 2024-09) and `tabled 0.21` → `tabled_derive 0.11` still depends on it, and no newer `tabled` release drops the dependency, so the workspace now vendors a patched copy at `vendor/proc-macro-error2` (the one-line rustc-suggested fix: `pub extern crate proc_macro`) pinned through `[patch.crates-io]` in the root `Cargo.toml`.
- New regression guard `scripts/tests/future-incompat_test.sh` (issue #446) fails the review-time checks whenever any dependency emits a future-incompat warning, so the warning cannot silently return if the patch is dropped or a new dependency regresses.
- `docs/workspace.md` documents the new guard, `docs/refactoring-module-split.md` drops the informational note about the warning, and `docs/wiki/what-works-now.md` removes the resolved `tabled`/`proc-macro-error2` work item.
- Version bumped 0.132.2 → 0.132.3 (patch — build hygiene fix).

## [0.132.2] — 2026-08-22

### Fix: `scripts/tests/rustdoc_test.sh` guard passes on `main` again (issue #443)

- The `MULTI_VALUED_PREDICATES` doc comment in `mimir-knowledge/src/graph/predicates.rs` no longer links to the private `is_favourite_family_predicate` helper; the helper is now a backtick code span, matching the documented convention that private items are never intra-doc link targets (`docs/connectors-framework.md`).
- Fixing that error surfaced a second pre-existing guard failure: the `memory_refresh_handler` doc comment in `mimir-server/src/routes/memory.rs` linked to `HookEngine::force_run` with no `HookEngine` import in scope, so the link now uses the full `mimir_core::hooks::HookEngine::force_run` path.
- The guard script and `docs/workspace.md` now list issue #443 alongside the earlier guard regressions (#276, #310, #337, #348).
- Version bumped 0.132.1 → 0.132.2 (patch — build/doc fix).

## [0.132.1] — 2026-08-22

### Fix: `retrieve_context` test proves the request-resolved LLM is used, and `docs/tools-registry.md` describes write-tool helpers accurately (issue #441)

- `test_chat_executes_retrieve_context_through_registry` now resolves a distinct request LLM via a model override, asserting all request-path calls land on that backend and the startup backend receives none, so a factory that captures the startup LLM instead of `ctx.llm` cannot pass.
- `docs/tools-registry.md` now describes `is_write_tool(name)` as a write-capability predicate, the `export_*` helpers as filtering the exported tool set, and the incognito execution guard as the separate responsibility of `ToolRegistry::execute`.
- Version bumped 0.132.0 → 0.132.1 (patch — test hardening and documentation fix).

## [0.132.0] — 2026-08-22

### Refactor: `retrieve_context` dispatches through the `ToolRegistry` like every other tool (issue #441)

- The chat route no longer special-cases `mimir_knowledge::RetrieveContextTool`: `ToolRegistry` gained a `ToolContext` (request-resolved LLM + incognito write-tool policy) and factory registration (`register_native_with_factory` / `register_with_factory`), so the tool is rebuilt per request with the request-resolved LLM (model/temperature overrides) and executes through the same path as all other tools.
- Permission checks (Auto/Ask/Disabled) and the incognito write-tool guard now apply uniformly inside `ToolRegistry::execute`; the route's duplicated permission lookup and incognito guard were removed.
- `ToolRegistry::execute` now takes `&ToolContext`; the retrieval agent, the `research_synthesis` skill, and registry tests pass an explicit context.
- `ToolContext::new` added, and `docs/tools-registry.md` updated to document the context, factory registration, and the new `execute` signature.
- Version bumped 0.131.7 → 0.132.0 (minor — refactor).

## [0.131.7] — 2026-08-21

### Fix: `docs/config-hot-reload.md` describes the watcher filename filter as a suffix match instead of exact match (issue #438)

- The File Watcher section stated config changes are filtered by filename with `ends_with("config.toml")`, but `spawn_config_watcher` compares the event path's file name for exact equality against the config file's file name, so a differently named file such as `myconfig.toml` does not trigger a reload. The wording now documents the exact-match behaviour.
- Version bumped 0.131.6 → 0.131.7 (patch — documentation fix).

## [0.131.6] — 2026-08-21

### Fix: `Mimir-Implementation-Context.md` no longer carries a manually-maintained VISION file count (issue #433)

- The context header's `**Vision Docs:**` line stated `VISION/` contained "48 files, 10 sections", but the directory has since grown to 50 Markdown files, so the count had drifted. The exact file count is dropped in favour of a stable section list (`00-Overview` through `09-Roadmap`), which does not require manual maintenance when VISION documents are added or removed.
- Version bumped 0.131.5 → 0.131.6 (patch — documentation fix).

## [0.131.5] — 2026-08-21

### Fix: reflow `docs/fact-extraction-pipeline.md` to the single-line prose standard (issue #432)

- `docs/fact-extraction-pipeline.md` no longer violates the AGENTS.md single-line prose standard: the adjacent `**Issue #136:**` / `**Issue #401:**` blockquote field-list entries after the predicate-resolution section are split into separate blockquote paragraphs (blank `>` line between entries) per `scripts/md-reflow`'s field-list rule, so the `md-reflow --check` guard no longer flags the file. Content is unchanged — only blockquote structure (whitespace-collapsed diff equality per `scripts/md-reflow`).
- Version bumped 0.131.4 → 0.131.5 (patch — documentation fix).

## [0.131.4] — 2026-08-21

### Fix: `docs/personality-system.md` no longer references a non-existent `PersonalityPreset` type (issue #426)

- The "Module Design" section's `### PersonalityPreset` heading is renamed to `### Built-In Presets`, matching the actual implementation (built-in presets are private helper methods on `Personality` in `mimir-core/src/personality.rs`, not a standalone type). Content is unchanged — only the misleading heading is corrected.
- Version bumped 0.131.3 → 0.131.4 (patch — documentation fix).

## [0.131.3] — 2026-08-21

### Fix: reflow `VISION/03-Connectors/User-Experience.md` to the single-line prose standard (issue #418)

- `VISION/03-Connectors/User-Experience.md` no longer violates the AGENTS.md single-line prose standard: the hard-wrapped paragraph after the connector-add wizard transcript (drift from PR #385) is merged back into a single flowing line, so the `md-reflow --check` guard no longer flags the file. Content is unchanged — only line wrapping (whitespace-collapsed diff equality per `scripts/md-reflow`).
- Version bumped 0.131.2 → 0.131.3 (patch — documentation fix).

## [0.131.2] — 2026-08-21

### Fix: PR #442 review feedback — full-queue data loss, shutdown waiter race, and test-wait scoping

- `connector_item.remember` full-queue rejections no longer drop the staged email: when the hook's pending queue is full, `extract` records the message in the Email connector's durable ledger as a bounded queue-overflow entry (raw RFC 822 bytes base64-encoded, capped at 1024 mirroring the queue cap) instead of letting the IMAP cursor advance past an un-enqueued message. Every extraction cycle drains the overflow back into the staged buffer and re-attempts the enqueue, and a restart re-stages it from the persisted durable state; the ledger's pending map now also covers this overflow path (the legacy pre-hooks drain is unchanged).
- `HookEngine::shutdown` registers the dispatch-loop exit waiter (`Notify::notified`) before sending the shutdown signal, so a prompt loop exit can no longer race ahead of the waiter and leave shutdown waiting out the full 5-second timeout.
- The non-canonical-predicate connector test now waits for both `pending_depth() == 0` and `running_count() == 0` (the handler can still be inserting after the queue drains), and the server incognito-test idle wait is scoped to the `remember.chat` pending queue only, so an unrelated running hook (e.g. `memory.condensation` under the fast-learning config) can no longer make it flaky.
- Docs updated: `docs/hooks.md`, `docs/email-connector.md`, and the wiki email-connector page now describe the durable queue-overflow path (the stale pre-hooks "512 KiB / 32 pending messages" wording is gone).
- Version bumped 0.131.1 → 0.131.2 (patch — bugfixes).

## [0.131.1] — 2026-08-21

### Fix: PR #442 review feedback — job-queue documentation accuracy

- `docs/wiki/job-queue.md` now separates scheduler rules from per-hook queue policies instead of claiming all background jobs share one pipeline: scheduled jobs (nightly optimization, pending-fact cleanup, events scan) follow the scheduler's dedupe/debounce/cooldown/idle-gate lifecycle, while `remember.chat` (per-session debounce, idle-gated), `memory.condensation` (scheduler debounce/cooldown, idle-gated), and `connector_item.remember` (FIFO, ungated) each apply their own hook policy.
- `docs/job-queue.md` corrects the typed job identifier documentation: `DaemonJob` covers only `knowledge.optimization`, `knowledge.pending_cleanup` and `events.upcoming_scan_{idx}` are plain `Job` registrations, and `JobQueue::run_now`/`JobQueue::status` accept the persistent job ID as `&str`.
- Version bumped 0.131.0 → 0.131.1 (patch — documentation fix).

## [0.131.0] — 2026-08-21

### Feature: hooks engine — typed background tasks with per-hook queue policies (issue #386)

- Added a typed hooks engine in `mimir-core/src/hooks/`: a minimal `Trigger` enum (`TurnCompleted`, `ConnectorItemStaged`, `FactInserted`), per-hook `QueuePolicy` (`Multiple` FIFO, `SingularFirstWins`, `SingularLastWins` with debounce + payload merge), `KeyScope` (`Global`/`PerKey`), execution `Gate` (`IdleGated` cooldown + LLM-pool idle, `Ungated`), and `RetryPolicy` with capped exponential backoff. A single dispatch loop drains the in-memory pending queue through the durable `JobQueue`; each registered hook owns its durable job whose handler executes the currently running instance via a `Weak` reference. `force_run` bypasses all gates for manual refreshes, and `pending_depth`/`pending_depth_for`/`running_count` provide observability.
- Migrated remembering and memory condensation onto hooks: `remember.chat` (per-session `SingularLastWins` debounced by `agent.remember_debounce_seconds`, default 10, idle-gated, non-incognito only, fired on both blocking and streaming chat paths, with a bounded retry budget for transient extraction failures), `connector_item.remember` (per-item `Multiple` FIFO, ungated, with the Email connector's retry/terminal-failure policy moved into the hook runner and terminal failures recorded durably in the shared `ProseRetryLedger`), and `memory.condensation` (global `SingularLastWins`, idle-gated, replacing the dirty-signal scheduler submission; `POST /memory/refresh` force-runs it).
- Removed the `remember` tool from the registry and the personality operating directives; the `remember_tool_schema` remains as the extraction schema for the hook pipeline. Incognito turns never enqueue any hook (asserted by server integration tests).
- Surfaced the pending hook queue depth as `hook_queue_depth` in `GET /status`.
- DRY: the LLM-pool idle check shared by the scheduler and the hooks engine now lives in one `LlmBackend::pool_is_idle` default method.
- Docs: new `docs/hooks.md` + `docs/wiki/hooks.md`; updated `docs/learning-orchestration.md`, `docs/fact-extraction-pipeline.md`, `docs/librarian-agent.md`, `docs/personality-system.md`, `docs/tools-registry.md`, `docs/chat-server.md`, `docs/email-connector.md`, `docs/job-queue.md`, `docs/memory-system.md`, `docs/nightly-optimization.md`, `docs/shutdown.md`, `README.md`, `Mimir-Implementation-Context.md`, and the wiki pages (personality, librarian, tools, server, knowledge-graph, fact-extraction, connectors, memory, job-queue, configuration, what-works-now, daemon-shutdown, cli-commands).
- Version bumped 0.130.6 → 0.131.0 (minor — new feature).

### Fix: PR #442 review feedback — bounded pending queues, shutdown handshake, and durability accuracy

- `Hook::max_pending` bounds `QueuePolicy::Multiple` pending depth (the Email connector registers `connector_item.remember` with a 1024-instance cap); over-capacity triggers return a new `TriggerStatus::QueueFull` and the connector logs the drop instead of retaining raw email payloads without bound. Staged emails are now consumed by value so the raw RFC 822 bytes move into the hook payload instead of being cloned.
- `HookEngine::shutdown()` now awaits the dispatch loop's exit (via an internal exit signal) after cancelling the in-flight run, so the terminal `job_runs` status is written before the SQLite pool closes; the dispatch loop also checks the shutdown signal before starting any new run. Timed-out durable runs are requeued as retryable failures instead of silently dropped.
- `ChatLearningHandler`'s turn merge keeps the accumulated transcript when a malformed payload arrives; `confirm_fact` now sets the condensation dirty signal so a confirmed sensitive fact re-ranks condensed memory; malformed-message hook failures are recorded durably in the `ProseRetryLedger`; `ConnectorSupervisor` always injects its own knowledge graph into connector factories (the `with_knowledge_graph` builder is gone); the email hook registration is gated on the `gmail` feature; `StatusResponse::hook_queue_depth` gained `#[serde(default)]` so a new CLI works against older daemons; the Email connector's dependency injection was grouped into a named `EmailConnectorDeps` struct.
- Tests: incognito persistence checks now observe the configured user's facts after the hook queue drains, the non-canonical-predicate test waits on queue drain instead of a fixed sleep, and new unit tests cover pending-capacity rejection, shutdown cancellation/finalisation, timeout requeue, malformed-message ledger durability, condensation-dirty on confirm, and older `/status` payloads.
- Docs: `docs/hooks.md`, `docs/shutdown.md`, `docs/memory-system.md`, `docs/fact-extraction-pipeline.md`, `docs/knowledge-graph-schema.md`, `docs/job-queue.md`, and the wiki pages (hooks, memory, cli-commands, configuration, librarian-agent, personality, server, what-works-now) corrected for the actual hook entrypoint, unit-payload contract, restart durability, LLM-pool-idle gating, environment-variable override, custom-preset behaviour, and incognito persistence.

## [0.130.6] — 2026-08-21

### Fix: PR #437 review feedback (deterministic config watcher regression tests)

- The two issue #415 config-watcher regression tests in `mimir-server/src/server.rs` now synchronise with successful watcher registration before dropping the runtime or rewriting the watched config. `spawn_config_watcher` gained a test-only readiness signal (`spawn_config_watcher_with_readiness`, compiled only under `cfg(test)`) emitted after `debouncer.watch` succeeds: the runtime-drop test waits for it before dropping the runtime and the reload test waits for it before writing the replacement content, so neither test can pass without exercising the registered watch — tokio does not guarantee that a `spawn_blocking` closure has started by the time the spawning call returns, so the previous fixed 300 ms delay could let both tests pass vacuously.
- Docs updated: `docs/config-hot-reload.md`, `docs/unit-tests.md`, `docs/wiki/Testing-and-Benchmarks.md`.
- Version bumped 0.130.5 → 0.130.6 (patch — test hardening).

## [0.130.5] — 2026-08-21

### Fix: daemon config watcher thread leaks and hangs runtime shutdown on error paths (issue #415)

- The config hot-reload watcher in `mimir-server/src/server.rs` now ties the `spawn_blocking` thread's lifetime to its async relay task through a `std::sync::mpsc` lifetime channel: the sender is owned by the async task, so the blocking loop observes `Disconnected` and exits whenever the task exits — on every exit branch, and when runtime teardown drops the task on an error path where the shutdown watch never fires (a panic, or an early return before the shutdown broadcast). Previously the blocking loop only exited via the `stop` flag set by the async task's shutdown branch, so a runtime dropped without the broadcast leaked the thread and tokio's runtime drop hung indefinitely joining the blocking pool.
- The watcher was extracted into a testable `spawn_config_watcher` helper; it subscribes to the shutdown watch before spawning the task and checks the current value before entering its loop (same pattern as the SIGHUP handler, issue #421).
- Added regression tests: dropping a runtime that spawned the watcher without firing the shutdown watch completes instead of hanging, and a content change on the config file is debounced, forwarded, and reloaded.
- Docs updated: `docs/config-hot-reload.md`, `docs/shutdown.md`, `docs/unit-tests.md`, `docs/wiki/daemon-shutdown.md`, `docs/wiki/Testing-and-Benchmarks.md`, `docs/wiki/what-works-now.md`.
- Version bumped 0.130.4 → 0.130.5 (patch — bug fix).

## [0.130.4] — 2026-08-21

### Fix: PR #436 review feedback (event overlay scan coverage and documentation)

- `docs/events-reminders.md` now states that both the one-time and recurring branches of the Upcoming render apply the `fact_status_id NOT IN (Superseded, Forgotten)` filter, matching the queries — the one-time branch excludes `Superseded`/`Forgotten` facts in addition to the terminal-overlay suppression.
- The supersession regression test now pins the one-time scan path: past-due `AutoCompleteOnDate` overlays on directly-`Superseded` and directly-`Forgotten` facts are neither auto-completed by the scan nor dropped (both stay `Active`), alongside the existing recurring-path assertions.
- The dedup regression test now seeds `pending_event_meta` for the merged duplicate and asserts the row is removed alongside the overlay dismissal.
- Version bumped 0.130.3 → 0.130.4 (patch — test coverage and documentation).

## [0.130.3] — 2026-08-21

### Fix: superseded facts retire their event overlays (issue #413)

- `queries::fact::status::set_status_tx` now retires a fact's event overlay when the fact transitions to `Superseded`: any non-terminal overlay (`Pending`/`Active`/`Snoozed`) is dismissed (`status_id = Dismissed`, `addressed_at` set) and any persisted `pending_event_meta` row is deleted; `Completed` overlays are preserved as historical records. Because the retirement lives in the shared status transition, every supersession path stays in sync — the insert pipeline's overlap resolution (`queries/fact/conflict.rs`, which now delegates its status change to `set_status_tx` instead of duplicating the UPDATE + audit), the inference engine's contradiction rule, user status edits via `update_fact_status`, and the nightly dedup merge (`optimization/passes.rs`), which retires the merged duplicate's overlay directly.
- The scan queries (`get_active_recurring`, `get_past_due_auto_complete`) and the Upcoming render's recurring branch now join `facts` and exclude `Superseded`/`Forgotten` facts (`fact_status_id NOT IN (5, 6)`) as a second line of defense, so a stale overlay can never advance, auto-complete, or surface even if a future supersession path forgets to retire it.
- Added regression tests: a corrected recurring event retires the old overlay (dismissed, not advanced by the scan, only the corrected date surfaces in Upcoming), a directly-superseded fact's overlay is neither advanced nor surfaced, a `Completed` overlay survives supersession untouched, and the nightly dedup merge retires the merged duplicate's overlay.
- Version bumped 0.130.2 → 0.130.3 (patch — bug fix and test coverage).

## [0.130.2] — 2026-08-21

### Fix: PR #435 review feedback (connector predicate rendering)

- `purchased_from` facts now render in passive voice (`Order was purchased from Shop`), matching migration `053`'s seeded definition ("Subject was purchased from an organization"), and the memory renderer's exact-output pin now covers all 16 connector-emitted predicates, including the previously unpinned `attending`, `took_photo_at`, `departs_from`, `arrives_at`, `operated_by`, and `purchased_from` branches.
- Corrected the `0.130.1` changelog entry to list the full connector-emitted grammar.
- Version bumped 0.130.1 → 0.130.2 (patch — render phrase fix and test coverage).

## [0.130.1] — 2026-08-21

### Fix: PR #435 review feedback (connector predicate seeding)

- The deterministic memory renderer now covers the complete connector-emitted predicate grammar (`has_event`, `attending`, `took_photo_at`, `took_photo`, `has_flight`, `departs_from`, `arrives_at`, `operated_by`, `has_booking`, `has_order`, `purchased_from`, `has_delivery`, `shipped_by`, `delivered_to`, `has_ticket`, `issued_by`), with the exact render output pinned in tests.
- The Photos extraction test now asserts exactly which predicates each path emits (`took_photo_at` with GPS resolution, `took_photo` without), and the `oauth`/memory test counts in `docs/unit-tests.md` were reconciled with the suite (326 connector lib tests, 204 knowledge lib tests, 13 memory tests).
- Migration `053` now starts each WHERE clause on its own line (SQLFluff LT14), and the connector docs qualify that seeded constraints apply where applicable (typed entity-object shapes; literal-object predicates such as `took_photo` stay unconstrained).
- Version bumped 0.130.0 → 0.130.1 (patch — review fixes, test pins, and documentation).

## [0.130.0] — 2026-08-21

### Seed connector-emitted predicates as canonical ontology (issue #412)

- Added migration `053`: the 16 predicates the connectors emit deterministically (`has_event`, `attending`, `took_photo_at`, `took_photo`, `has_flight`, `departs_from`, `arrives_at`, `operated_by`, `has_booking`, `has_order`, `purchased_from`, `has_delivery`, `shipped_by`, `delivered_to`, `has_ticket`, `issued_by`) are now seeded canonical rows with descriptions, self-aliases, and subject/object constraints mirroring the emit sites, so a connector sync never silently auto-creates a `relationship_types` row on first use. The seed is name-keyed (upgrades canonicalise pre-existing auto-created rows in place) and reconciles unreferenced auto-created vocabulary like migration `050`.
- Added the public `CONNECTOR_EMITTED_PREDICATES` const and `is_canonical_predicate_name` helper in `mimir-knowledge` (re-exported at the crate root), and extended `CANONICAL_PREDICATES` with the new verbs so the strict conversational resolver accepts them too.
- The email LLM extraction layer now validates the LLM-emitted `relationship_type` against the canonical vocabulary and drops non-canonical predicates with a warning instead of letting them auto-create rows; the tool-schema description now cites only canonical examples.
- Added the seed pin in both directions: `mimir-connectors` tests assert every predicate emitted by the iCal, JSON-LD, and Photos extractors is canonical vocabulary (including the email LLM layer), and `mimir-knowledge` pins every registered connector predicate to a seeded canonical row with its constraint pair (`connector_emitted_predicates_are_seeded_canonical`).
- Added render templates for the new predicates in the deterministic memory/upcoming renderer, and updated the predicate ontology docs (`docs/knowledge-graph-schema.md`, `docs/fact-extraction-pipeline.md`, `docs/email-connector.md`, `docs/unit-tests.md`, wiki pages).
- Version bumped 0.129.1 → 0.130.0 (minor — additive ontology seed, migration, and public API surface).

## [0.129.1] — 2026-08-21

### Fix: PR #434 review feedback (category memory buckets)

- `POST /kb/categories` now returns the same `CategoryResponse` shape as the list and detail routes, so an unset `memory_bucket_id` is omitted instead of serialised as `null`.
- The client KB tests now pin the `memory_bucket_id` wire contract: list and detail fixtures assert the decoded bucket id, and the create test sends `memory_bucket_id: 3` and matches it in the POST request body.
- Corrected `docs/memory-system.md` to state that only `Identity` gets a reserved first fill phase — `Upcoming`, `Relationships`, `Preferences`, and `General` are filled together by score, and the lowest bucket id is the multi-category classification rule rather than a general fill-priority control.
- Version bumped 0.129.0 → 0.129.1 (patch — response-shape consistency, test coverage, and documentation).

## [0.129.0] — 2026-08-20

### Refactor: memory bucket classification is now data-driven (issue #407)

- Added migration `052`: a `memory_buckets` lookup table (Identity, Upcoming, Relationships, Preferences, General) and a `categories.memory_bucket_id` column backfilled from the taxonomy seeded in migration `031` (identity 100–199, upcoming 900–999, relationships 400–499, preferences 300–399 plus the outliers 570/670/680/690/830/870, everything else general). Bucket ids are ordered by priority, so a fact tagged with several categories resolves to `MIN(c.memory_bucket_id)` — the highest-priority bucket.
- Removed the hard-coded category ID ranges and preference-extras list from `mimir-knowledge/src/queries/memory/`; the memory query now reads the bucket from the category row and `ranking.rs` only maps a stored bucket id to the `MemoryBucket` enum (falling back to General for unset or unknown ids). Adding, renaming, or re-parenting a category can no longer silently change memory bucketing.
- Added `memory_bucket_id` (optional) to the category model, `kb category add --memory-bucket-id` CLI flag, and the `POST /kb/categories` body / category responses, so runtime-created categories can opt into a bucket instead of defaulting to General; unknown bucket ids are rejected with a validation error instead of a foreign-key failure.
- Added tests pinning every seeded category to its expected bucket (migration test), a multi-category priority integration test, and unit tests for the bucket-id mapping.
- Version bumped 0.128.2 → 0.129.0 (minor — refactor with an additive API field).

## [0.128.2] — 2026-08-20

### Bugfix: `memory.temporal_horizon` now drives the upcoming-events horizon (PR #431 review)

- The chat, status, and memory routes passed a literal 30 days to `render_upcoming_section`, so changing `memory.temporal_horizon` (or `MIMIR_MEMORY_TEMPORAL_HORIZON`) had no effect. All three routes now read the live config snapshot (`cfg.memory.temporal_horizon`), matching the existing `MemoryCondenser` wiring for `condensation_top_n`.

### Docs: PR #431 review corrections

- Corrected the condensation contract in `Mimir-Implementation-Context.md` and `VISION/01-Core-Agent/Memory-System.md`: the LLM receives deterministic text rendered from the schema, while `condensation_top_n` (default 500) is a cache-invalidation control — a hash of the top-N fact IDs and scores gates when the LLM call is skipped.
- Corrected the prompt refresh boundary in `VISION/01-Core-Agent/Memory-System.md`, `docs/memory-system.md`, and the Phase-2 roadmap: the memory-bearing system prompt is composed at session creation for non-incognito sessions and reused for the session's lifetime, while incognito requests build a fresh prompt per request (not injected before each turn).
- Unified the ranking formula across the Phase-2 migration documents (`VISION/02-Knowledge-Graph/Phase-2-Design-Discussion.md` section K and roadmap section 2.14) to include the shipped priority and centrality factors: `confidence × category_weight × temporal_boost × priority × centrality`.
- Renamed the roadmap success criterion to state that the legacy file-backed `memory.md` system was removed in v0.37.0 (issue #111) and the Knowledge Graph became the sole memory store — no data migration ran.
- Updated the `MIMIR_MEMORY_TEMPORAL_HORIZON` description in `docs/wiki/configuration.md` to describe the upcoming-events horizon it now controls.
- Version bumped 0.128.1 → 0.128.2 (patch — bugfix plus documentation corrections).

## [0.128.1] — 2026-08-20

### Docs: stale memory.md references removed from project context and VISION docs (issue #406)

- Rewrote the memory sections of `Mimir-Implementation-Context.md` to describe the live knowledge-graph condensation pipeline: ranking formula (`confidence × category.memory_weight × temporal_boost × priority × centrality`), LLM condensation with deterministic Rust fallback, the upcoming-events section, and the regeneration triggers. The architecture diagram and daemon component tree no longer reference `memory.md` or `MemoryManager`/`MemoryLoader` (deleted in v0.37.0, issue #111), and the Phase 1 goal, roadmap summary row, and config paths section were updated (the CLI table already matched the live pipeline).
- Rewrote `VISION/01-Core-Agent/Memory-System.md` to match the implemented KG-backed memory system (replacing the deleted `memory.md` design), following the same pattern as the VISION personality rewrite (issue #389).
- Updated `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` section 2.15 and the success criteria to record the landed migration: `memory.md` was removed outright in v0.37.0 (issue #111), the one-time seed and `MemoryManager` facade refactor never shipped (now documented as obsolete rather than pending), and the success criterion is marked complete.
- Added a landed-status note to `VISION/02-Knowledge-Graph/Phase-2-Design-Discussion.md` section K, and corrected the stale "Key facts I know about you:" phrasing in `docs/memory-system.md` and the Phase-2 roadmap to the actual system prompt header ("Core facts about the user (condensed subset — not a complete picture; treat as starting context, not exhaustive)").
- Version bumped 0.128.0 → 0.128.1 (patch — documentation update).

## [0.128.0] — 2026-08-20

### Refactor: single source of truth for multi-valued predicate list splitting (issue #405)

- The extraction prompt no longer carries the BAD/GOOD list-splitting parsing example — it keeps a single high-level rule ("Emit one fact per list item") — and the Rust splitter (`split_list_objects` in `mimir-knowledge/src/extract/parse.rs`) is the one deterministic enforcement mechanism, per the project's "logic in Rust, not in prompts" rule.
- The 11-predicate allow-list is consolidated into the single `MULTI_VALUED_PREDICATES` constant in `mimir-knowledge/src/graph/predicates.rs`, re-exported at the crate root and from `queries::fact` (the old path is preserved). Both the list splitter and the insert overlap logic (`insert_fact_in_tx`) read from it, so the split set and the multi-valued coexist semantics can never drift apart; a new test pins every multi-valued predicate as canonical.
- The splitter now handles the open `favourite_<thing>` family generically via the shared `is_favourite_family_predicate` helper (the same shape check the strict resolver uses), so `favourite_movie → "Inception, Interstellar"` splits into two facts instead of one comma-joined value — closing the drift between the prompt's `favourite_{{thing}}` standard and the Rust splitter. The same helper marks the family multi-valued in `insert_fact_in_tx`, so the split facts coexist as active facts instead of the second superseding the first (pinned by the extraction integration test).
- Tests: unit coverage for the `favourite_*` split in `parse.rs`, a full-pipeline integration test in `extraction_tool_test.rs`, a prompt-shape test in `prompt_tests.rs`, and the canonical-subset pin in `predicate_allowlist_test.rs`.
- Docs updated in `docs/fact-extraction-pipeline.md`, `docs/wiki/fact-extraction.md`, and `docs/wiki/what-works-now.md`.
- Version bumped 0.127.0 → 0.128.0 (minor — refactor plus the backwards-compatible `favourite_*` splitting extension).

## [0.127.0] — 2026-08-20

### Refactor: consolidate redundant KG predicates and seed the relationship-type DAG (issue #403)

- Migration `051_consolidate_predicates_and_seed_dag.sql` consolidates the overlapping predicate vocabulary: `based_in` and `lived_in` are now aliases of a single `resides_in` (current vs previous residence is one relation with different `valid_from`/`valid_until` bounds), and `is_in` is an alias of `located_in` (both express physical containment). Existing facts, constraints, and hierarchy edges are repointed name-keyed (never id-assumed), and the old names keep resolving through the alias table, so stored queries and callers are unaffected.
- The same migration seeds the dormant relationship-type DAG with four query-only abstract parents — `employment` → `works_at`/`works_as`/`job_title`, `education` → `studied`/`studied_at`/`completed_degree`/`educational_status`, `residence` → `resides_in`, `containment` → `located_in` — so `kg_query --include-subtree` expresses real generalisation. The parents are excluded from the Rust `CANONICAL_PREDICATES` allow-list: the strict conversational resolver rejects them as fact predicates, while `kg_query` resolves them through the alias table for subtree expansion.
- Rust updates: `CANONICAL_PREDICATES` in `mimir-knowledge/src/graph/predicates.rs` swaps `based_in`/`lived_in`/`is_in` for `resides_in`; the extraction prompt and `remember` tool description instruct `resides_in`; the transitivity inference rule keys on `located_in`; the memory renderer adds a `resides_in` line; and the audit-log filter now resolves predicate names through the alias table so filtering by `is_in`/`based_in`/`lived_in` still matches facts stored under the consolidated verbs.
- Tests: new seeded-DAG and consolidation-alias coverage in `relationship_type_dag_test.rs`, a seeded-parent `kg_query --include-subtree` test in `relationship_subtree_test.rs`, strict-resolver rejection of abstract parents and alias resolution in `predicate_allowlist_test.rs`, and updated seed counts/lookup pins in `migrations_test.rs`, `lookup_sync_test.rs`, and `relationship_ontology_test.rs`; fixtures that hardcoded the old `is_in` id (1) now resolve `located_in` dynamically.
- Docs updated in `docs/knowledge-graph-schema.md`, `docs/inference-engine.md`, `docs/benchmarks.md`, `docs/wiki/knowledge-graph.md`, `docs/wiki/categories-and-aliases.md`, `docs/wiki/kg-tools.md`, `docs/wiki/inference-rules.md`, `docs/wiki/facts.md`, and `docs/wiki/what-works-now.md`.
- Version bumped 0.126.1 → 0.127.0 (minor — refactor plus the additive migration and the alias-resolving audit filter).

## [0.126.1] — 2026-08-20

### Bugfix: seeded relationship-type constraints are enforced on every insert path (issue #402)

- The seeded `relationship_constraints` allow-list (migration 013, renamed by 031) is now enforced at the write boundary: `validate_predicate_in_tx` in `mimir-knowledge/src/queries/entity/predicates.rs` is called from `insert_fact_in_tx` and the pending-confirmation path `insert_sensitive_fact`, so single inserts, batch inserts, conversational extraction, connector syncs, and inference cascades all reject entity-object facts whose (subject, object) entity-type pair is not in the seeded set (e.g. `born_on` with a non-DateTime object, or `has_partner` with a Place object) with the renamed `KnowledgeError::InvalidRelationshipConstraint` before any overlap/supersession side effects.
- The public typed validator `validate_predicate` was fixed to match the seeded permissive contract: predicates without constraint rows (auto-created and connector-emitted types, tracked by issues #403/#412) accept any entity types, and literal-object facts carry no object type so they always pass. Previously the validator rejected every unseeded predicate and was dead code in production — it had zero call sites.
- The new `InvalidRelationshipConstraint` error maps to HTTP 400 `VALIDATION_ERROR` with its descriptive message in `mimir-server`, so a rejected combination surfaces as a client error instead of a masked 500.
- Tests: new `mimir-knowledge/tests/relationship_constraint_test.rs` (8 tests) pins the contract on the single-insert, batch, shared-normalize, sensitive (pending-confirmation), and public-validator paths; existing fixtures that used nonsense combinations (`fact_crud_test.rs` `works_as` Person→Person, `inference_tests.rs` Place-subject `visited` facts) now use constraint-valid entity types while preserving their original intent.
- Docs updated in `docs/knowledge-graph-schema.md`, `docs/fact-extraction-pipeline.md`, `docs/wiki/knowledge-graph.md`, and `docs/wiki/what-works-now.md`.
- Version bumped 0.126.0 → 0.126.1 (patch — bug fix; the error-variant rename only touches an internal error enum with no production callers).

## [0.126.0] — 2026-08-20

### Bugfix: conversational extraction enforces the canonical predicate allow-list (issue #401)

- The chat extraction path (`extracted_to_normalized` in `mimir-knowledge/src/extract/pipeline.rs`) now resolves predicates through the new `KnowledgeGraph::resolve_canonical_relationship_type`, which enforces the Rust-side `CANONICAL_PREDICATES` allow-list: seeded canonical predicates and their aliases resolve as before, the prompt-instructed `favourite_<thing>` family is accepted, and any other predicate (e.g. an LLM-invented `moved_into`) is rejected with a clear `Validation` error instead of auto-creating a `relationship_types` row. Per-fact errors are still tolerated, so one bad predicate never aborts the batch.
- Migration `050_seed_canonical_predicates_and_reconcile.sql` seeds the 13 predicates the extraction path legitimately uses that were missing (`skill`, `has_appointment`, and the sensitive predicates migration `029` intended to mark: `allergy`, `medication`, `diagnosis`, `income`, `salary`, `password`, `ssn`, `social_security_number`, `bank_account`, `credit_card`, `insurance`), bringing the canonical set to 44, and deletes auto-created relationship types that no fact references (cascading to their aliases, constraints, and hierarchy edges). Auto-created types with facts are preserved; repointing them onto canonical predicates is the ontology consolidation's job (issues #403/#412). The `favourite_<thing>` open set requires a non-empty thing, so a bare `favourite_` is rejected like any other unknown predicate.
- The shared `normalize_and_insert` boundary remains permissive for connector-provenance facts — connector-emitted predicates (`has_event`, `attending`, `took_photo_at`, email LLM predicates) are tracked by the ontology consolidation work.
- Tests: new `mimir-knowledge/tests/predicate_allowlist_test.rs` (12 tests) covers invented-predicate rejection, batch tolerance, alias/favourite resolution, strict-resolver behaviour (including rejection of a bare `favourite_` prefix), the allow-list-to-seed pin, and the reconciliation migration (including cascade cleanup of orphaned aliases); the old auto-creation tests in `extraction_text_fallback_test.rs` were inverted to assert rejection, and chat-path tests using non-canonical predicates (`lives_in`, `lives_at`) now use the canonical `based_in`.
- Docs updated in `docs/fact-extraction-pipeline.md`, `docs/knowledge-graph-schema.md`, `docs/wiki/fact-extraction.md`, and `docs/wiki/what-works-now.md`.
- Version bumped 0.125.2 → 0.126.0 (minor — bug fix plus the new public `resolve_canonical_relationship_type` API and the additive migration).

## [0.125.2] — 2026-08-20

### Docs: VISION personality doc matches the implemented preset system (issue #389)

- Rewrote `VISION/01-Core-Agent/Personality.md` to describe the implemented design: the four built-in presets, custom `<name>.personality.md` files in `~/.config/mimir/personalities/`, preset selection via config/env/CLI/REPL/API, Rust-composed operating directives and core-facts block, and custom-overrides-built-in semantics.
- The unimplemented `personality.toml` sections, tone knobs (`style`/`verbosity`/`proactive_tone`/`humor`), proactive phrase overrides, and `context.public`/`context.private` are now documented as non-goals, with cross-references to `docs/personality-system.md` and the planned discovery work (issue #387).
- Updated the stale personality lines in `Mimir-Implementation-Context.md` and the Phase 1 roadmap (system prompt no longer references the deleted `memory.md`; it is composed of a preset, operating directives, and condensed knowledge-graph memory).
- Version bumped 0.125.1 → 0.125.2 (patch — documentation update).

## [0.125.1] — 2026-08-20

### Bugfix: workspace test suite is isolated, deterministic, and hang-proof (issue #384)

- The daemon-down CLI tests in `mimir/tests/cli_tests.rs` no longer rely on "nothing is listening at the default base URL": each test points `MIMIR_BASE_URL` at the never-bindable loopback endpoint `http://127.0.0.1:0` (`unreachable_daemon_base_url` in `mimir/tests/common/mod.rs`) plus temp HOME/XDG dirs, so a real or leftover daemon on the configured port can no longer flip the assertions (previously `test_status_fails_when_server_down`, `test_stop_when_server_down`, and `test_memory_fails_when_server_down` panicked whenever a daemon was reachable).
- `shutdown::tests::test_server_exits_after_stop` is fully isolated: it now injects a known API token and a mock LLM via `start_server_with_llm` (no reads/writes of the real `~/.local/share/mimir/api_token`) and points the context/knowledge/scheduler DBs at temp paths, so the test can no longer hold handles on the real `knowledge.db`/`jobs.db` (the sqlite-lock hang seen in full-suite runs, also tracked as issue #396). The spawned server task is owned by a kill-on-drop guard so a panicking assertion cannot leak a live server into parallel suites.
- `TestDaemon` (the in-process daemon fixture for the `mimir` E2E tests) kills its server on drop, so a test that panics before `stop()` cannot leak a daemon holding the reserved port and temp DBs.
- Verified with a fake daemon serving real `/status` JSON at the CLI's resolved base URL: the bare CLI succeeds (the old test failure mode), while the new isolated `cli_tests` binary still passes. The full `cargo test --workspace` run is green and completes in about a minute when compiled (the earlier ~15-minute figure was dominated by cold compilation; rate-limit/backoff and OAuth E2E waits had already been shortened to deterministic millisecond-scale durations).
- Docs updated in `docs/e2e-testing.md`, `docs/unit-tests.md`, `docs/wiki/Testing-and-Benchmarks.md`, and `docs/wiki/what-works-now.md`.
- Version bumped 0.125.0 → 0.125.1 (patch — bug fix).

## [0.125.0] — 2026-08-20

### Bugfix: kb audit and fact-detail endpoints render the same changed_by wire strings (issue #380)

- The `kb audit` endpoint previously surfaced the raw lowercase `changed_by_types.name` lookup column (`user`, `system`, `inference_engine`, `nightly_optimization`) via the SQL join in `mimir-knowledge/src/queries/audit.rs`, while `GET /kb/facts/{id}` rendered the enum `ChangedBy::as_str()` variant strings (`User`, `System`, `InferenceEngine`, `NightlyOptimization`) — so the same audit entry rendered differently depending on the endpoint. `AuditLogRow` now carries `changed_by_id` instead of the joined `name` column, and the `kb audit` route resolves the wire string through the same `ChangedBy::try_from(i16)` + `as_str()` helper as the fact-detail route (single source of truth, issue #358).
- Tests: `query_audit_log_filtered` now asserts the `changed_by_id` discriminant, `test_kb_audit_returns_entries` asserts the `User` wire string, a new `test_kb_audit_and_show_render_same_changed_by_casing` integration test pins identical `changed_by` casing across both endpoints for the same content-update audit entry, and the `mimir-api-types` audit-row roundtrip fixture now uses the canonical `User` wire string.
- Docs updated in `docs/knowledge-graph-schema.md`, `docs/fact-management.md`, and `docs/wiki/facts.md`.
- Version bumped 0.124.4 → 0.125.0 (minor — bug fix plus the `AuditLogRow` `changed_by_name` field removal, a breaking change to an internal crate API acceptable per the project's internal-API policy; mirrors the 0.110.0 precedent).

## [0.124.4] — 2026-08-20

### Bugfix: SIGHUP config-reload handler is registered before its task is spawned (issue #369)

- The SIGHUP hot-reload handler in `mimir-server/src/server.rs` is now registered synchronously by a dedicated `spawn_sighup_reload_handler` helper, before the tokio task is spawned: `tokio::signal::unix::signal()` installs the libc handler in its constructor, so a SIGHUP arriving in the window between spawn and the task's first poll (e.g. during startup under parallel load) is caught and triggers a config reload instead of hitting the default disposition and killing the daemon. This mirrors the SIGTERM/SIGINT registration in `spawn_os_signal_shutdown` (issue #329) and closes the same startup race. The handler task holds the shutdown watch sender so the channel stays open for the task's lifetime; no behaviour change otherwise.
- Regression test: a child-process test (`server::tests::test_sighup_registered_before_spawn_reloads_config`) sends a real SIGHUP to an isolated re-executed child immediately after `spawn_sighup_reload_handler` returns and asserts the config is reloaded from disk — with the old in-task registration the child dies from the default disposition (signal 1), exactly as the original bug did.
- Docs updated in `docs/config-hot-reload.md`, `docs/wiki/configuration.md`, and `docs/unit-tests.md` (mimir-server 44 → 45 lib tests).
- Version bumped 0.124.3 → 0.124.4 (patch — bug fix).

## [0.124.3] — 2026-08-19

### Bugfix: calendar KB tests wait for the event overlay, closing the initial-cycle flake (issue #367)

- The `mimir-connectors` calendar knowledge-graph tests (`calendar_kb_tests.rs`) no longer break their initial-cycle wait on the first non-empty fact list. The events overlay is inserted by `insert_event_if_absent` in a separate transaction after the fact commits, so under full-suite parallel load a test could observe the committed fact before the overlay commit and panic at `.expect("overlay")` on `get_event_by_fact`. Both tests now share a `wait_for_has_event_overlay` helper that polls `get_event_by_fact` until the overlay is queryable, closing the race the deterministic tombstone trigger from #320 did not cover and removing the duplicated wait-loop code (DRY).
- Verified with 20/20 repeated `calendar_kb_tests` runs and a full `cargo test -p mimir-connectors --all-features` pass.
- Docs updated in `docs/calendar-connector.md`, `docs/wiki/Testing-and-Benchmarks.md`, and `docs/wiki/what-works-now.md` (the last open flaky-test issue is closed, so the "Known Limitations" flaky-tests row is removed).
- Version bumped 0.124.2 → 0.124.3 (patch — bug fix).

## [0.124.2] — 2026-08-19

### Docs: add change_types seeds to the schema-doc lookup-seeding list (issue #364)

- `docs/knowledge-graph-schema.md` now lists migrations `027` (`Rejected = 8`) and `034` (`ContentUpdate = 9`) in the lookup-seeding migration list, closing the same class of drift that #306 fixed for `023`/`024`: both migrations seed `change_types` rows that map to `ChangeType` enum variants via `#[repr(i16)]` discriminants, and the list's stated criteria already covered them.
- Version bumped 0.124.1 → 0.124.2 (patch — documentation update).

## [0.124.1] — 2026-08-19

### Bugfix: wizard secrets are prompted exactly once (issue #399)

- The interactive `mimir connector add` wizard's hidden secret prompts (Gmail/CalDAV OAuth client secrets and app passwords) no longer ask for a second masked "Confirmation:" input. inquire 0.9.4 enables password confirmation by default, so after the first masked entry a second hidden prompt appeared — with no visible keystrokes it looked like the wizard froze right before the OAuth browser opened, and a mismatch produced an unexplained error loop. Secrets are typically pasted, the mismatch loop is more confusing than a rare typo, and the connector auth step already fails loudly with a clear error when a secret is wrong, so confirmation is now disabled (`without_confirmation()`).
- Regression test: the production password-prompt configuration is pinned so wizard secrets are asked exactly once (confirmation disabled), alongside the existing scripted-prompt wizard tests.
- Docs updated in `docs/cli.md`, `docs/wiki/cli-commands.md`, `docs/unit-tests.md` (connector 51 → 52, bin 83 → 84), and `docs/wiki/Testing-and-Benchmarks.md` (bin 83 → 84).
- Version bumped 0.124.0 → 0.124.1 (patch — bug fix).

## [0.124.0] — 2026-08-19

### Feature: interactive `mimir connector add` wizard

- `mimir connector add` with no arguments now runs an interactive wizard instead of failing on missing required arguments: it lists the daemon's supported `(connector_type, backend)` pairs from the live catalog for selection, prompts for the display name (defaults to the type) and slug (defaults to the slugified name), asks the per-backend questions with sensible defaults, and drives authentication. Gmail IMAP is offered with OAuth browser login first (Google authorization/token endpoints pre-filled — the user supplies their own OAuth client ID and, for confidential clients, an optional client secret prompted hidden; the CLI prints the authorize URL and opens the browser, and the URL can also be opened manually in a browser on the same machine, since the PKCE callback binds to `127.0.0.1`) and an app-password fallback; CalDAV offers app password or OAuth; local backends (Photos) need no credential. The wizard runs on the same shared register+ingest core as the flag form, so a canceled prompt or aborted OAuth flow still exits with nothing created.
- The wizard requires a terminal: with piped stdin it fails fast with a pointer to the flag form (`mimir connector add gmail --backend imap …`), which remains available for scripts — only non-OAuth flows with supplied credentials are fully non-interactive, while OAuth still opens the browser for PKCE and waits for the loopback callback. Partial arguments (`type` without `--backend`, or vice versa) now produce a friendly hint instead of a bare clap error.
- The created instance is read-only by design: connectors only import data from the service, and write-back runs only via an explicit `mimir connector act <slug>`; the wizard's summary says so. Credentials — app passwords and OAuth client secrets — are stored by the daemon's secret store (per-slug `0600` files in a `0700` directory, fail-closed on loosened permissions) and are never written into `config_json`; empty or whitespace-only app passwords are rejected before registration.
- Tests: scripted-prompt driver exercises the full wizard path (catalog → prompts → PKCE/app-password → register → token ingest) in unit tests, plus binary-level CLI tests for the non-TTY guard and partial-argument hints. New coverage: the OAuth client secret is carried through the credential bundle and kept out of `config_json` (CLI unit + daemon ingest tests), and empty/whitespace-only app passwords are rejected in both the Gmail and CalDAV flows. Docs updated in `docs/cli.md`, `docs/email-connector.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, `docs/wiki/email-connector.md`.
- Version bumped 0.123.0 → 0.124.0 (minor — new feature).

## [0.123.0] — 2026-08-19

### Remove the autonomous development loop from the repository

- The autonomous development loop was a local-only developer tool and never belonged in the codebase: `scripts/autonomous-loop.sh`, its systemd units (`scripts/systemd/mimir-autonomous.{service,timer}`), its test (`scripts/tests/autonomous-loop_test.sh`), and the technical/user docs (`docs/autonomous-loop.md`, `docs/wiki/autonomous-loop.md`) are removed from the repository. The tool now lives locally at `~/.local/share/mimir/autonomous-loop/` (script, systemd units, and tests), with the script defaulting to `~/Projects/Rust/Mimir` (overridable via `MIMIR_REPO`) and the systemd service updated to point at the local copy.
- All references to the loop are removed from `AGENTS.md` (section renamed to "Issue Hygiene"), `README.md` (Autonomous Development Loop section dropped), and `CHANGELOG.md` (historical entries for the loop removed).
- Version bumped 0.122.2 → 0.123.0 (minor — removal of a developer-tooling subsystem).

## [0.122.2] — 2026-08-19

### Docs: refresh the stale mimir binary unit-test count (issue #362)

- `docs/unit-tests.md` reported 29 bin tests for the `mimir` binary and listed only the `kb/` module; the actual inline unit-test count is 69 (verified via `cargo test -p mimir --bin mimir`), split across `connector/` (37), `kb/` (15 — the previous doc figure of 16 was also off by one), `daemon_guard.rs` (10), `chat.rs` (6), and `cli_util.rs` (1). The `mimir` section now lists every test-carrying module with its count and coverage summary.
- `docs/wiki/Testing-and-Benchmarks.md` is reconciled with `docs/unit-tests.md`: the `mimir` bin figure moves from 15 → 29 to 15 → 69, and the `mimir-knowledge` (195 → 204), `mimir-server` (41 → 44), and `mimir-connectors` (319 → 321) lib-test counts in the same sentence are refreshed to the verified workspace counts. `docs/wiki/what-works-now.md` version header is bumped to 0.122.2.
- Version bumped 0.122.1 → 0.122.2 (patch — documentation update).

## [0.122.1] — 2026-08-19

### Docs: clarify the lookup-name vs wire-string contract in knowledge-graph-schema (PR #381 review)

- `docs/knowledge-graph-schema.md` now states that the `ChangeType` / `ChangedBy` / `EntityType` enum conversions align the lookup identifiers across storage and the API/tool contracts (via `TryFrom<i16>` discriminants), while endpoint string representations may differ — e.g. audit SQL responses report `changed_by` as the lowercase lookup name (`user`), whereas fact-detail output uses the title-case variant string (`User`).
- Version bumped 0.122.0 → 0.122.1 (patch — documentation update).

## [0.122.0] — 2026-08-19

### Refactor: typed enum→wire-string mapping for all remaining lookup enums (issue #358)

- `mimir-knowledge` now exposes `ChangeType::as_str()` / `ChangedBy::as_str()` (`models::audit_log`) and `EntityType::as_str()` (`models::entity`) with the stable wire names (`"created"`...`"content_update"`, `"User"`/`"System"`/`"InferenceEngine"`/`"NightlyOptimization"`, `"Person"`...`"DateTime"`), plus `TryFrom<i16>` for all three and a case-insensitive `FromStr` for `ChangeType` and `EntityType`.
- `mimir-server` KB helpers (`change_type_name`, `changed_by_name`) and the audit filter parser in `kb_audit_handler` now map through the typed conversions instead of magic numbers; this fixes the live bug where a `ChangeType::ContentUpdate = 9` audit row rendered as `"Unknown"` in the fact-detail API even though the audit filter accepts `"content_update"`.
- `mimir-knowledge` LLM-facing helpers (`fact_status_name`, `source_type_name`, `entity_type_name` in `tools/`), the extraction validator `parse_entity_type`, and the `kg_search` entity-type filter now delegate to the enum conversions, keeping the `Unknown({id})` tool fallback.
- Added round-trip unit tests asserting the wire names match the enum variants (mimir-knowledge) and the route helpers' output contract (mimir-server), plus a server integration test proving a content-update audit entry renders as `"content_update"` in the fact-detail API.
- Docs updated: `docs/unit-tests.md`, `docs/fact-management.md`, `docs/knowledge-graph-schema.md`, `docs/wiki/facts.md`.
- Version bumped 0.121.5 → 0.122.0 (minor — refactor plus bug fix).

## [0.121.5] — 2026-08-19

### Fix: mimir-connectors oauth refresh helpers dead-code warnings under oauth-only feature combinations (issues #351, #374)

- `cargo check -p mimir-connectors --all-targets --no-default-features --features oauth` (and `--features test-mock-oauth`) emitted four dead-code warnings: the `refresh::resolve_access_token` re-export in `src/oauth/mod.rs` and the `REFRESH_SKEW_SECS` constant / `refresh_token` / `resolve_access_token` helpers in `src/oauth/refresh.rs` are only called by the Calendar and Email backends and the refresh module's own unit tests, so the `oauth`-only combination (e.g. the CLI PKCE flow, A4 / #205) compiled them as dead code. The helpers and their imports are now cfg-gated to `any(feature = "calendar", feature = "gmail", test)` and the `mod.rs` re-export to `any(feature = "calendar", feature = "gmail")`, matching their actual callers, so every supported feature combination compiles warning-free.
- Regression guard: `scripts/tests/no-default-features_test.sh` now runs the whole `--no-default-features` feature matrix (not just the no-features lib target) with `RUSTFLAGS="-D warnings"`, so no supported combination can regress to dead-code or unused-import warnings; this also resolves the follow-up tracked in #374 (the `oauth`-only combo the guard's old scoping note excluded).
- Docs updated: `docs/connectors-framework.md` (oauth module gating), `docs/workspace.md` (regression guards), `docs/wiki/what-works-now.md` (#349 backlog row removed — already fixed in 0.117.0 by commit 42e7d86 — and version header).
- Version bumped 0.121.4 → 0.121.5 (patch — build hygiene fix).

## [0.121.4] — 2026-08-19

### Docs: fix the last mimir-server rustdoc intra-doc link warnings (issue #348)

- `cargo doc -p mimir-server --no-deps --all-features` emitted two unresolved-link warnings: `mimir-server/src/routes/connectors.rs` module doc linked `[ConnectorRegistry]`, which is not in scope in that module (the type lives in `mimir_connectors` and is re-exported at its crate root), so the link now uses the full path `[mimir_connectors::ConnectorRegistry]`; and `mimir-server/src/state/builder.rs` linked `[MockLlmClient](mimir_core::llm::mock::MockLlmClient)`, but the `mock` module is cfg-gated behind `#[cfg(any(test, feature = "mock-llm"))]` and `mock-llm` is only a mimir-server dev-dependency, so the link could never resolve in a doc build — the sentence now names the mock client in plain text, matching the #337 fix for the same link class in `mimir-core`.
- Regression guard: `scripts/tests/rustdoc_test.sh` now also builds `mimir-server` docs with `RUSTDOCFLAGS="-D warnings" --all-features` (the "widen this check once #348 lands" note from the #337 fix), so the whole workspace's intra-doc links stay warning-free at review time.
- Docs updated: `docs/workspace.md` (regression guards), `docs/wiki/what-works-now.md` (#348 backlog row removed, version header).
- Version bumped 0.121.3 → 0.121.4 (patch — documentation fix).

## [0.121.3] — 2026-08-18

### Docs: refresh stale `job_queue/` test count in docs/unit-tests.md (issue #345)

- The `mimir-core` `job_queue/` bullet reported 15 tests, but the module now carries 23 (`mimir-core/src/job_queue/tests.rs` has 19, `resources.rs` has 4). Refreshed the bullet to 23 and listed the cgroup resource-limit helper tests it now covers; the surrounding `mimir-core` total (279 lib tests) was already accurate.
- Version bumped 0.121.2 → 0.121.3 (patch — documentation update).

## [0.121.2] — 2026-08-18

### Docs: remove duplicate `> **Scope:**` line in docs/config-system.md (issue #344)

- The header blockquote of `docs/config-system.md` contained two `> **Scope:**` entries; the first was stale (it omitted `mimir-core/src/paths.rs`). Removed the stale line, so the blockquote lists the complete, current scope once.
- Version bumped 0.121.1 → 0.121.2 (patch — documentation update).

## [0.121.1] — 2026-08-18

### Fix: mimir-connectors dead-code warning for connector_fact under --no-default-features (issue #342)

- `cargo build -p mimir-connectors --no-default-features` emitted `warning: function connector_fact is never used` because the shared `NormalizedFact` constructor (`mimir-connectors/src/fact.rs`, issue #255) is only used by the feature-gated Photos, iCal VEVENT (Calendar + Email iMIP), and Email JSON-LD backends plus its own unit tests. The `fact` module is now cfg-gated to `any(feature = "photos", feature = "calendar", feature = "gmail", test)`, matching its callers, so the always-compiled framework core stays warning-free under `--no-default-features`.
- Regression guard: `scripts/tests/no-default-features_test.sh` now additionally checks the no-features lib target with `RUSTFLAGS="-D warnings"` so the framework core cannot regress to dead-code warnings.
- Docs updated: `docs/connectors-framework.md` (connector_fact + crate layout), `docs/workspace.md` (regression guards).
- Version bumped 0.121.0 → 0.121.1 (patch — bug fix).

## [0.121.0] — 2026-08-18

### Refactor: shared auth-method discriminant trait (issue #341)

- The `discriminant()` mapping (auth variant → serde `kind` tag string) was implemented verbatim on both `CalendarAuthMethod` (`mimir-connectors/src/calendar/mod.rs`) and `EmailAuthMethod` (`mimir-connectors/src/email/config.rs`), with the same doc comment. Both enums now implement the shared `crate::secrets::AuthMethodDiscriminant` trait instead, so a new auth kind must implement the same `discriminant()` contract in both connectors, and each connector's mapping is pinned against its serde `kind` tag by unit tests; the shared mismatch error (issue #273) can never silently diverge from the stored-config `kind`.
- **Tests.** New `auth_method_discriminants_match_serde_kind_tag` tests in the Email and Calendar config suites assert every variant's `discriminant()` equals its serde `kind` tag.
- **Docs.** `docs/connector-secret-store.md`, `docs/calendar-connector.md`, `docs/unit-tests.md`, `Mimir-Implementation-Context.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.120.3 → 0.121.0 (minor — refactor).

## [0.120.3] — 2026-08-18

### Fix: flaky photos failure-cycle test under parallel load (issue #339)

- `mimir-connectors/tests/photos_connector.rs` `failed_before_extract_cycle_does_not_duplicate_staged_photo` flaked under `cargo test --workspace` parallel load: the `FailFirstExtractPhotosConnector` wrapper recorded the retry extract result with `store`, which is last-writer-wins — a second, overlapping `extract()` call that found the already-drained buffer overwrote the counter back to 0 after the first batch was already ingested, so the poll loop observed the fact in the knowledge graph while the counter read 0.
- The wrapper now accumulates with `fetch_add` so a later empty extract cannot erase the count of an earlier successful one; a deterministic regression test (`retry_fact_count_accumulates_across_overlapping_extracts`) reproduces the overlapping-extract race directly.
- Docs updated: `docs/photos-connector.md` (failure-cycle testing notes), `docs/wiki/what-works-now.md` (version header).
- Version bumped 0.120.2 → 0.120.3 (patch — bug fix).

## [0.120.2] — 2026-08-18

### Docs: fix the last mimir-core rustdoc intra-doc link warnings (issue #337)

- `mimir-core/src/llm/backend.rs` doc comment linked `[MockLlmClient](super::mock::MockLlmClient)`, but the `mock` module is cfg-gated behind `#[cfg(any(test, feature = "mock-llm"))]`, so the link never resolved in a plain `cargo doc` build; the sentence now names the mock client in plain text.
- `mimir-core/src/skills/markdown.rs` `parse_skill_file` doc linked `[MAX_SKILL_FILE_SIZE]`, a private const, which rustdoc rejects for public docs; the link is now plain code formatting, and duplicated doc lines on `SkillFrontmatter` and `parse_skill_file` were removed.
- Regression guard: `scripts/tests/rustdoc_test.sh` now also builds `mimir-core` docs with `RUSTDOCFLAGS="-D warnings"` for both the default feature set and `--all-features` (the `mock-llm`-gated surface); the remaining workspace crate with doc warnings is tracked in #348 (mimir-server).
- Docs updated: `docs/workspace.md` (regression guards), `docs/wiki/what-works-now.md` (backlog: #337 row removed, version header).
- Version bumped 0.120.1 → 0.120.2 (patch — documentation fix).

## [0.120.1] — 2026-08-18

### Fix: SIGTERM during daemon startup no longer kills the process (issue #329)

- `spawn_os_signal_shutdown` registered the SIGTERM/SIGINT handlers inside the spawned task, so a signal arriving before the task's first poll hit the default disposition and terminated the daemon — the `e2e_sigterm_exits_promptly` flake under parallel load, where the health listener became ready before the signal task was scheduled. The handlers are now registered synchronously via `tokio::signal::unix::signal()` before the task is spawned, so a SIGTERM/SIGINT arriving once the listener is accepting always takes the graceful path.
- Added a deterministic regression test (`test_sigterm_registered_before_spawn_returns`) that sends SIGTERM to an isolated child process immediately after `spawn_os_signal_shutdown` returns and asserts the shutdown trigger fires; without the fix the child is killed by the default disposition (signal 15), reproducing the flake's failure mode. The signal is sent to a child rather than to the test process itself because tokio's OS-signal listeners are process-global: a SIGTERM delivered to the test process would also fire the SIGTERM/SIGINT listeners that other tests in the same binary install via `serve_with_bounded_drain`, shutting their servers down mid-test.
- The `nix` dev-dependency in `mimir-server` now declares the `process` feature directly (alongside `signal`) because the regression test calls `nix::unistd::getpid()`; previously it relied on feature unification from `mimir-core` (PR #370 review).
- Docs updated: `docs/shutdown.md` (trigger architecture), `docs/wiki/daemon-shutdown.md` (startup signal handling), `docs/wiki/what-works-now.md` (version header).
- Version bumped 0.120.0 → 0.120.1 (patch — bug fix).

## [0.120.0] — 2026-08-18

### Refactor: deterministic `ConnectorRegistry` accessors (issue #322)

- `ConnectorRegistry::registered_types()` is removed: it returned connector types in hash-seed-dependent order and had zero callers once `pairs()` (issue #271) superseded it for discovery — the catalog route and CLI surface use `pairs()`, which is sorted by type then backend.
- `ConnectorRegistry::backends_for()` now returns backend names sorted alphabetically, so every registry accessor is order-stable; the registry-dispatch test asserts the sorted contract directly instead of sorting in the test.
- Docs updated: `docs/connectors-framework.md` (registry API listing and determinism note), `Mimir-Implementation-Context.md` (F7 method list), `docs/wiki/what-works-now.md` (version header; dropped the stale #271 "no connector catalog" known-limitation row — the catalog landed in #271).
- Version bumped 0.119.7 → 0.120.0 (minor — refactor).

## [0.119.7] — 2026-08-18

### Docs: narrow scheduler-immunity claim for the deterministic tombstone-cycle test (PR #366 review)

- `docs/connectors-framework.md` previously claimed the whole CalDAV server-side-deletion test is "immune to scheduler load", but only the explicitly triggered tombstone cycle is deterministic: `trigger_sync_by_slug` preempts the polling interval and awaits the full cycle, while the test's initial cycle still waits on the connector's first polling run. The sentence now scopes the claim to the tombstone cycle and notes the initial-cycle wait.
- Version bumped 0.119.6 → 0.119.7 (patch — documentation fix).

## [0.119.6] — 2026-08-18

### Fix: calendar KB server-side-deletion test no longer flakes under parallel load (issue #320)

- `mimir-connectors/tests/calendar_kb_tests.rs::calendar_server_side_deletion_trashes_facts_and_hides_upcoming_event` waited on the CalDAV connector's 1 s polling interval for the tombstone cycle with a fixed 8 s deadline, which could be missed under full-suite parallel load. The test now drives the tombstone cycle deterministically: the connector's `poll_interval_secs` is set to 3600 so no automatic cycle races the single-use wiremock window, and the test calls `ConnectorSupervisor::trigger_sync_by_slug`, which preempts the polling interval and returns only after the full cycle (sync → extract_deletions → trash → cursor persist) completed. The post-trigger assertions are no longer at the mercy of scheduler load, and the test runs ~5x faster (~0.25 s vs ~1.3 s).
- Docs updated: `docs/connectors-framework.md` (deterministic multi-cycle test pattern), `docs/wiki/what-works-now.md` (version header).
- Version bumped 0.119.5 → 0.119.6 (patch — bug fix).

## [0.119.5] — 2026-08-18

### Docs: fix the last mimir-knowledge rustdoc intra-doc link warning (issue #310)

- `mimir-knowledge/src/events.rs` module docs linked `[get_overdue_events]` bare, which no longer resolves since the item moved to `queries::event::get_overdue_events` in the module split (0.94.0); the link now uses the full path `crate::queries::event::get_overdue_events`, so `cargo doc -p mimir-knowledge --no-deps` builds with zero warnings.
- Regression guard: `scripts/tests/rustdoc_test.sh` now also builds `mimir-knowledge` docs with `RUSTDOCFLAGS="-D warnings"` (mimir-knowledge has no feature flags, so the default build is the full surface); the remaining workspace crates with doc warnings are tracked in #337 (mimir-core) and #348 (mimir-server).
- Docs updated: `docs/workspace.md` (regression guards), `docs/wiki/what-works-now.md` (backlog: #337 added).
- Version bumped 0.119.4 → 0.119.5 (patch — documentation fix).

## [0.119.4] — 2026-08-18

### Docs: fix Phase-2 roadmap and schema-doc spec drift (issue #306)

- `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` now lists the seeded `entity_types` taxonomy (including `Activity(7)` from migration `001` alongside `DateTime(8)` from migration `012`), adds `Contradicts(4)` to `relation_types` (migration `024`) and `Geographic(6)` to `location_types` (migration `046`), marks `entity_dates` / `entity_date_types` as dropped by migration `040` and superseded by the events overlay (migration `039`), and reflects the events-overlay reality in the 2.2 Entity Management checklist.
- `docs/knowledge-graph-schema.md` adds migrations `023` and `024` to the lookup-seeding list: `023` re-seeds `preference_categories` (7 rows) and `preference_source_types` (3 rows), matching the Rust enums in `mimir-knowledge/src/models/preference.rs`; `024` seeds the `Contradicts(4)` `relation_types` variant.
- Version bumped 0.119.3 → 0.119.4 (patch — documentation).

## [0.119.3] — 2026-08-18

### Docs: reconcile inline unit-test counts in Testing-and-Benchmarks (PR #361 review)

- `docs/wiki/Testing-and-Benchmarks.md` now reports the same inline unit-test counts as `docs/unit-tests.md` (mimir-api-types 63, mimir-client 74, mimir-core 279, mimir-knowledge 195, mimir-server 41), verified against `cargo test --workspace`.
- Version bumped 0.119.2 → 0.119.3 (patch — documentation).

## [0.119.2] — 2026-08-17

### Docs: document mimir-connectors unit-test coverage (issue #299)

- `docs/unit-tests.md` gains a `mimir-connectors` section summarising the crate's 319 inline lib tests (email extraction/LLM layers, OAuth PKCE/refresh, photos, supervisor, rate-limit, calendar, geocoder, ical, secrets, `ConnectorContext`, and the shared `test_utils` doubles from #290/#298), notes that the `test-mock-oauth`-gated mock OAuth server (#207) is pinned by the E2E suites rather than inline tests, and adds `cargo test -p mimir-connectors --lib` to the running commands.
- Docs updated: `docs/unit-tests.md`, `docs/wiki/Testing-and-Benchmarks.md`.
- Version bumped 0.119.1 → 0.119.2 (patch — documentation).

## [0.119.1] — 2026-08-17

### Docs: enforce the single-line markdown prose standard (issue #294)

- The one-off reflow of all repo `.md` files landed earlier via #245 (the `scripts/md-reflow` tool); this change wires the tool's `--check` mode into the repo's review-time regression guards as `scripts/tests/md-reflow_test.sh`, so any future hard-wrap drift fails at review time instead of accumulating.
- Docs updated: `scripts/md-reflow/README.md`, `docs/workspace.md` (regression guards).
- Version bumped 0.119.0 → 0.119.1 (patch — documentation and tooling).

## [0.119.0] — 2026-08-17

### Refactor: typed enum→wire-string mapping for KB route helpers (issue #293)

- `mimir-knowledge` now exposes `SourceType::as_str()` / `FactStatus::as_str()` (wire names), `TryFrom<i16>` for both enums, and a case-insensitive `FromStr` for `FactStatus`; `Fact::status()` delegates to `TryFrom` instead of a hand-rolled match.
- `mimir-server` KB route helpers (`source_type_name`, `status_name`, `parse_status`) now map through the typed conversions instead of magic numbers, keeping the wire strings byte-identical (`"UserEdit"`...`"System"`, `"Active"`...`"Forgotten"`, lowercase status input) and falling back to `"Unknown"` for unknown IDs.
- Added round-trip unit tests asserting the wire names match the enum variants (`mimir-knowledge`) and the route helpers' output contract (`mimir-server`).
- Docs updated: `docs/unit-tests.md`, `docs/fact-management.md`.
- Version bumped 0.118.0 → 0.119.0 (minor — refactor).

## [0.118.0] — 2026-08-17

### Refactor: async user-skill loading and config test-module cleanup (issue #287)

- `SkillRegistry::load_user_skills` now performs asynchronous file I/O (`tokio::fs::read_dir` / `read_to_string`) instead of blocking the runtime with synchronous `std::fs` calls, so a future server-side skill surface can load skills without blocking the daemon. The CLI caller and the `mimir-core` integration tests were updated to `.await`, and a new test covers the missing-directory no-op. A missing directory is still a no-op returning `0`, while other I/O errors now propagate to the caller instead of being silently treated as an empty directory.
- Removed the duplicate `#[cfg(test)]` attribute on the test module in `mimir-core/src/config/reload.rs`.
- Docs updated: `docs/skills.md` (user-skill loading), `docs/wiki/what-works-now.md` (backlog row removed).
- Version bumped 0.117.3 → 0.118.0 (minor — refactor).

## [0.117.3] — 2026-08-17

### Docs: HTAB separator described as an implementation extension (PR #355)

- Documentation, test comments, and the changelog now state that RFC 7235 separates the bearer scheme from the credentials with one or more SP characters, and that accepting a tab (HTAB) as well is an interoperability extension of the daemon's `require_auth` rather than RFC 7235 syntax.
- Version bumped 0.117.2 → 0.117.3 (patch — documentation update).

## [0.117.2] — 2026-08-17

### Fix: RFC 7235 header parsing and CLI token-attachment fallback (issue #281)

- The daemon's `require_auth` now accepts a single tab (HTAB) as well as a space between the `Bearer` scheme and the credentials — an interoperability extension beyond RFC 7235's SP separator — with a regression test (`test_status_accepts_tab_separated_scheme`).
- The CLI's `make_client` now uses the fallible `try_new_with_token` constructor and, if the token cannot be attached as a header, prints a warning and falls back to a tokenless client so the daemon's `401` surfaces the problem instead of a panic. `mimir-client` exposes `DEFAULT_CONNECT_TIMEOUT` / `DEFAULT_TOTAL_TIMEOUT` so the default timeouts are defined once and reused by `new`, `with_token`, and the CLI.
- Docs updated: `docs/api-authentication.md` (HTAB parsing, CLI fallback, test list), `docs/wiki/server.md` (CLI warning behaviour).
- Version bumped 0.117.1 → 0.117.2 (patch — bug fixes).

## [0.117.1] — 2026-08-17

### Security: HTTP API authentication review fixes (PR #353)

- Token creation now publishes the token atomically (temporary file + hard link) and returns the canonical token when a concurrent creator wins, so the daemon and CLI can never end up with different tokens or observe a partial token file.
- Auth tests now assert the `WWW-Authenticate: Bearer` challenge for wrong and malformed credentials, not just missing ones.
- Docs corrected: the pre-authentication threat model distinguishes unguarded routes from loopback-gated ones, the constant-time claim is limited to token comparison, the protected-route curl examples present the token, `/chat` is no longer classified as read-only, and the token and loopback guard are described as separate controls.

## [0.117.0] — 2026-08-17

### Security: HTTP API authentication (issue #281)

- The daemon API had no authentication: any local process (or any other local user) could read the entire knowledge graph, forge chat turns, edit/forget facts, and delete connectors, and a `0.0.0.0` bind exposed the unguarded routes to the network (the loopback-gated routes stayed local-only). Every route except `GET /health` now requires a bearer token (`Authorization: Bearer <token>`), rejected with `401` + `WWW-Authenticate: Bearer` otherwise.
- The token is 256 bits of CSPRNG entropy (`getrandom`), hex-encoded, stored `0600` at `~/.local/share/mimir/api_token`, generated at `mimir init` and lazily by the daemon or CLI for existing installs, and never overwrites a user-supplied token. Comparison is constant-time (`subtle`).
- `mimir-client` gained `with_token` / `try_new_with_token` (default `Authorization` header on every request, SSE included); the CLI's `make_client` auto-discovers the token, so all commands work unmodified. `GET /health` stays unauthenticated as the daemon-guard liveness probe.
- The loopback guard remains as an independent control for destructive routes; a non-loopback bind relies on the token for authentication (loopback-gated routes still return `403` remotely) and logs a startup warning.
- Tests: `mimir-core` token-file unit tests, `mimir-server/tests/auth_tests.rs` (missing/wrong/malformed token, challenge header, health exception), a `mimir-client` header-attachment test, and an E2E test proving unauthenticated `401` while `mimir status` keeps working. All existing server route tests now present the shared test token.
- Docs updated: new `docs/api-authentication.md` (threat model, lifecycle, non-loopback guidance), `docs/chat-server.md`, `docs/wiki/server.md`, `docs/wiki/getting-started.md`, `docs/wiki/what-works-now.md`, `Mimir-Implementation-Context.md`, `VISION/08-Architecture/Deployment-Model.md`.
- Version bumped 0.116.3 → 0.117.0 (minor — new security capability).

## [0.116.3] — 2026-08-17

### Build: mimir-connectors test targets compile under every feature combination (issue #277)

- The shared integration-test fixtures in `mimir-connectors/tests/common/mod.rs` imported `CalendarConnector` and other `calendar`-gated helpers unconditionally, so `cargo test -p mimir-connectors --no-default-features --features test-mock-connector` (and any other non-`calendar` combination that compiles the supervisor tests) failed with an unresolved-import error. The calendar-specific imports and helpers are now gated behind `#[cfg(feature = "calendar")]`, while the framework/supervisor harness stays ungated, so `--no-default-features --all-targets` compiles the framework/mock-only test targets under every feature combination.
- Regression guard: `scripts/tests/no-default-features_test.sh` checks the `--no-default-features --all-targets` build matrix (no features, each backend/test-harness feature in isolation, and the calendar + mock combination) so newly-ungated fixtures fail at review time.
- Docs updated: `docs/connectors-framework.md` (test feature-gating convention + verification commands), `docs/wiki/what-works-now.md` (backlog: #277 done).
- Version bumped 0.116.2 → 0.116.3 (patch — build hygiene fix).

## [0.116.2] — 2026-08-17

### Docs: zero rustdoc warnings in mimir-connectors (issue #276)

- `cargo doc -p mimir-connectors --no-deps --all-features` emitted 35 warnings (28 unresolved intra-doc links, 6 private-item links, and one ambiguous-name warning). Every link now resolves: module-level `//!` docs use full paths (`crate::ical::parse_ical_to_vevents`, `crate::registry::ConnectorRegistry::create`, `mimir_knowledge::normalize::NormalizedFact`) or `Self::` where applicable, private items are rendered as code spans instead of links, and the `async_trait` ambiguity is disambiguated with `macro@async_trait`. Two feature-gated `mock` links in `lib.rs` and `secrets/memory.rs` were also fixed, so the doc build is clean under default, `--all-features`, and `--no-default-features`.
- Regression guard: `scripts/tests/rustdoc_test.sh` runs `RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-connectors --no-deps` under `--all-features` and `--no-default-features` so newly-introduced broken links fail at review time.
- Docs updated: `docs/connectors-framework.md` (documentation conventions section), `docs/wiki/what-works-now.md` (backlog: #276 done, #348 added for the remaining mimir-server warning).
- Version bumped 0.116.1 → 0.116.2 (patch — documentation fix).

## [0.116.1] — 2026-08-17

### KB daemon-route cleanup and loopback gating (issue #90)

- Issue #90's migration was already completed by issue #61 (PR #121): every `mimir kb` command talks to the daemon over HTTP and the CLI has no direct SQLite access. This change set removes the last leftover — the unused `mimir-knowledge` dependency in `mimir/Cargo.toml` (orphaned when the CLI moved to `mimir-client`).
- Destructive KB routes are now loopback-only, matching the existing guard on optimization run-now, pending confirm/reject, connector credentials, and stop: `POST /kb/facts/forget`, `DELETE /kb/trash`, and `POST /kb/trash/restore` reject non-loopback callers with `403 Forbidden`, so a daemon bound to a LAN address can be inspected but not mutated remotely. Read-only KB routes stay open.
- Tests: three new non-loopback rejection tests in `mimir-server/tests/kb_query_tests.rs`, and the forget/restore/trash roundtrip now attaches loopback `ConnectInfo`.
- Docs updated: `docs/chat-server.md` (loopback guard route list), `docs/wiki/server.md` (loopback-only routes), `docs/wiki/cli-commands.md` (destructive KB commands are local-only).
- Version bumped 0.116.0 → 0.116.1 (patch — cleanup and security hardening).

## [0.115.4] — 2026-08-16

### Docs: refresh stale connectors framework + Phase 3 roadmap docs (issue #274)

- `docs/connectors-framework.md` crate-layout table now lists the `secrets`, `rate_limit`, `geocoder`, `ical`, `calendar`, and `email` subsystems with their landing issues, the feature-flags block matches `mimir-connectors/Cargo.toml` (including `test-utils`, `test-mock-oauth`, and the `oauth` `dep:url` gate), and the status blockquote opens with "Implemented" instead of "Scaffolded".
- `VISION/09-Roadmap/Phase-3-Connectors.md` checklists now reflect the landed work (F1–F10, F12–F13 — F11 keyring deferred, C1–C7, A1–A4, T1–T2) with issue references, the stale "Duration: 6–8 weeks" header is replaced by a status blockquote pointing at `Phase-3-Plan.md` as the design source of truth, and the success criteria / risks sections note how each was met or mitigated.
- `docs/wiki/connectors.md` status blockquote updated to "Implemented"; `docs/wiki/what-works-now.md` maintenance backlog no longer lists #274 (and the closed #260).
- Version bumped 0.115.3 → 0.115.4 (patch — documentation update).

## [0.115.3] — 2026-08-16

### PR #343 review: remaining CodeRabbit findings

- `docs/connectors-framework.md` and `docs/photos-connector.md` now describe the daemon `AppState` supervisor wiring, the sync route/CLI, and the geocoder injection in present tense instead of as future work (A1–A3 / #202–#204).
- `mimir-connectors/tests/mock_connector.rs` module comment rewritten in present tense: the configurable mock surface (`MockConnector::from_config`, `MockFactConfig`, `MockSyncRecorder`) exists and the file is feature-gated for execution.
- `mimir-connectors/tests/scaffold_smoke.rs` no longer gates the whole file behind `test-mock-connector`; `registry_starts_empty` now runs in default test builds, with only the mock import and `mock_connector_reports_identity` feature-gated.
- Version bumped 0.115.2 → 0.115.3 (patch — documentation and test-gating fixes).

## [0.115.2] — 2026-08-16

### PR #343 review: finish `--` separator support in md-reflow

- `scripts/md-reflow` argument parsing was extracted into a unit-tested `parse_args` helper, and arguments after the conventional `--` separator are now treated as paths verbatim. The earlier fix still ran the flag filter over post-separator arguments, so a file named like `--weird.md` was dropped and the tool fell back to scanning the whole tree; `md-reflow --check -- --weird.md` now processes the explicit path. The `--check` non-zero exit for unreadable files (from the first review pass) is unchanged.
- Version bumped 0.115.1 → 0.115.2 (patch — CLI bug fix).

## [0.115.1] — 2026-08-16

### Docs: reflow all markdown prose to the single-line standard (issue #245)

- Every repo `.md` file now follows the AGENTS.md single-line prose standard: paragraphs and list-item continuations are single flowing lines, with blank lines only between blocks. A new `scripts/md-reflow` tool (pulldown-cmark-based) performs the reflow and offers a `--check` mode so future changes can verify compliance. Blockquote field-lists (one `> **Field:** value` entry per line) were restructured so each entry is its own blockquote paragraph; tables, fenced code blocks, nested lists, and code blocks are untouched. Content is unchanged — only line wrapping (verified by whitespace-collapsed diff equality per file).
- **Docs.** `docs/wiki/what-works-now.md` updated (reflow plus #245 removed from the maintenance backlog).
- **PR #343 review fixes.** `scripts/md-reflow` now fails `--check` on unreadable files, supports the `--` path separator, skips symlinks during the directory walk, enables definition-list parsing, and guards overlapping regions; the mock connector harness moved behind the off-by-default `test-mock-connector` feature; `docs/connectors-framework.md` status now reflects A1–A4 and C4–C7 as landed; minor prose corrections across `docs/` and `VISION/`.
- Version bumped 0.115.0 → 0.115.1 (patch — documentation update).

## [0.115.0] — 2026-08-16

### DRY: shared auth-method/secret-kind mismatch error (issue #273)

- The `auth method {} does not match stored secret kind` error arm was duplicated verbatim in the Calendar and Email connectors' `resolve_auth` matches (`mimir-connectors/src/calendar/credentials.rs` and `mimir-connectors/src/email/connector/credentials.rs`). Both now build the error through the shared `mimir_connectors::secrets::mismatch_error(discriminant)` helper, so the message text and the auth-kind `discriminant()` stay in sync across both backends. The message is unchanged and is now pinned by unit tests in both mismatch directions (the Calendar mismatch path previously had no test coverage, and the Email test only checked the error variant).
- **Tests.** New `secrets::tests::mismatch_error_pins_the_exact_message_text`; Calendar `resolve_auth_mismatch_reports_config_discriminant` and `resolve_auth_mismatch_oauth_config_with_app_password_bundle`; Email `auth_method_mismatch_is_an_error` strengthened to assert the exact text plus a new reverse-direction test.
- **Docs.** `docs/connector-secret-store.md`, `docs/calendar-connector.md`, `docs/wiki/connectors.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.114.3 → 0.115.0 (minor — refactor).

## [0.114.3] — 2026-08-16

### PR #338 review fix: never expose an in-progress backup as a completed `.db` file

- **Root cause.** `create_backup` reserved the final `knowledge-YYYY-MM-DD.db` path and ran `VACUUM INTO` directly into it, so once the copy wrote its first bytes the file was no longer zero-length and `prune_backups` treated it as a real backup; with more than seven concurrent runs a pruning pass could unlink an older in-progress backup, leaving the completed copy without a directory entry.
- **Fix.** `VACUUM INTO` now writes to a staging path (`knowledge-YYYY-MM-DD.db.staging`) that `prune_backups` never matches, and the completed backup is published to the reserved `.db` path with an atomic rename only after the copy succeeds. Failed runs remove both the reservation and any partial staging file; a staging orphan from a crashed run is cleared by the next run before `VACUUM INTO`.
- **Tests.** New unit tests cover the publish-without-leftovers contract, orphaned-staging cleanup, and pruning never removing staging files; the concurrent shared-backup-dir regression test now also asserts no staging files remain and every published backup is a queryable database.
- **Docs.** `docs/nightly-optimization.md` and `docs/wiki/nightly-optimization.md` updated.
- Version bumped 0.114.2 → 0.114.3 (patch — bug fix).

## [0.114.2] — 2026-08-16

### Flaky test fix: nightly-optimization backup race (issue #241)

- **Root cause.** `test_pending_confirmation_ttl_cleanup` (and two `inference_tests` cases) failed under parallel load because the test-only `run_nightly_optimization` helper hardcoded the real user data dir as the backup directory, and concurrent runs raced on it: `create_backup` picked its filename with a check-then-act sequence (`try_exists` + `VACUUM INTO`), so two runs could select the same file and one failed with `table _sqlx_migrations already exists` or `output file already exists`; `prune_backups` could also fail with `Io(NotFound)` when a concurrent run removed a file mid-scan.
- **Fix.** `run_nightly_optimization(kg, backup_dir)` now takes the backup directory explicitly and the three tests pass a per-test tempdir, so tests never write to the real user data directory. `create_backup` reserves its filename atomically (`O_EXCL` via `create_new`) so concurrent runs sharing a directory can never collide, and removes the reserved file if `VACUUM INTO` fails; `prune_backups` skips entries that vanish mid-scan instead of failing the pass and ignores empty reserved files left behind by a crash.
- **Tests.** New regression test `concurrent_full_runs_with_shared_backup_dir_do_not_corrupt_each_other` runs two full nightly pipelines concurrently against one shared backup dir and asserts both succeed and both write backups; `TestGraph` gains a `backup_dir()` helper.
- **Docs.** `docs/nightly-optimization.md` and `docs/wiki/nightly-optimization.md` updated.
- Version bumped 0.114.1 → 0.114.2 (patch — bug fix).

## [0.114.1] — 2026-08-16

### PR #336 review fixes

- **Cancellation documented as best-effort.** `JobQueue::cancel`/`cancel_all` only signal the run's `CancellationToken`; the dedicated run thread is neither aborted nor joined, so synchronous or blocking work can keep running until it finishes. `docs/job-queue.md` and `docs/wiki/job-queue.md` now describe this boundary, and `docs/job-queue.md` no longer claims user activity is the only non-cancellation path.
- **Run-now HTTP statuses.** `POST /memory/refresh` and `POST /kb/optimization/run-now` now return `409 Conflict` when the run was cancelled and `504 Gateway Timeout` when it timed out, instead of `200 OK` with a cancelled/timed-out summary body.
- **Spawn-failure cleanup.** If the dedicated job thread cannot be spawned, the `job_runs` row is finalized as `Failed` with `finished_at` and the spawn error persisted, so the run no longer stays `Running` across restarts.
- **Config widening.** `memory_limit_mb` is now `Option<u32>` so caps above 65535 MiB can be expressed.
- **Build/tests.** `nix` explicitly enables the `process` feature, and the Linux-only cgroup tests are gated to `target_os = "linux"` so `mimir-core` compiles on other platforms.
- Version bumped 0.114.0 → 0.114.1 (patch — review fixes).

## [0.114.0] — 2026-08-16

### JobQueue resource-limit enforcement and graceful cancellation (issue #91)

- **Graceful cancellation.** Each job run now carries a `tokio_util::sync::CancellationToken` exposed via `JobContext::cancellation_token()`; cooperative handlers `tokio::select!` on it at checkpoint boundaries and exit cleanly, and cancelled runs are recorded as `JobRunStatus::Cancelled` in `job_runs` (persisted across restarts). Cancellation is best-effort: the token only signals the run, and the dedicated run thread is neither aborted nor joined, so synchronous or blocking work can keep running until it finishes. `JobQueue::cancel(job_id)` cancels one running job, `JobQueue::cancel_all()` cancels every running job, and `BackgroundScheduler::shutdown()` calls `cancel_all()` so daemon shutdown never waits for a long-running job.
- **Best-effort per-job resource limits.** `Job::with_resource_limits(...)` attaches `JobResourceLimits` (`cpu_cores`, `nice_level`, `memory_limit_bytes`). Enforcement is OS-specific and never fails the job: Linux CPU affinity via `nix::sched`, POSIX `nice` via `rustix::process`, and a Linux cgroup v2 `memory.max` cap when the cgroup filesystem is writable. Each run executes on a fresh dedicated thread (named `mimir-job-<id>`) so thread-local limits are discarded on exit and never leak into pooled threads; the process-wide cgroup move is restored on drop.
- **Configuration.** `[knowledge.optimization]` now wires `cpu_cores` and `nice_level` (previously parsed but unused) into the optimization job, and gains an optional `memory_limit_mb` (default unset) for the best-effort cgroup v2 memory cap.
- **Tests.** `mimir-core` gains unit + integration coverage: cancellation of running jobs (cooperative and non-cooperative), `cancel_all`, cancelled-status persistence across queue reopen, scheduler shutdown cancelling an in-flight job, resource limits applying during a run (Linux), cgroup path parsing, and config parsing/defaults for `memory_limit_mb`.
- **Docs.** `docs/job-queue.md`, `docs/nightly-optimization.md`, `docs/wiki/job-queue.md`, `docs/wiki/nightly-optimization.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.113.0 → 0.114.0 (minor — backwards-compatible new feature).

## [0.113.0] — 2026-08-16

### Connector CLI: env/stdin secret ingestion (issue #270)

- **`mimir connector add` / `auth` no longer require `--password`/`--token` flags for non-interactive credential supply.** The flags leak secrets to the process list (`ps aux`) and shell history, so two channels that avoid the command line were added: `--password-stdin` / `--token-stdin` (the whole piped stream is the secret, trailing newlines stripped — `cat secret.txt | mimir connector add ... --password-stdin`, mirroring `docker login --password-stdin`) and the `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN` environment variables (read by the CLI only, never by the daemon; the value stays in the process environment, so load it from a protected source). Per-kind precedence is flag → stdin flag → env var → interactive `inquire` prompt; `auth` also infers the credential kind from the env vars (exactly one set) when neither config nor flags declare one, and the non-terminal error messages now point at all command-line-avoiding channels. The flags are kept for script convenience and the leak risk is documented.
- **Tests.** `mimir/tests/connector_cli_tests.rs` gains binary-level coverage: `--password-stdin` ingestion for `add` and `auth` (piped secret asserted on the daemon's token route), `MIMIR_CONNECTOR_TOKEN` / `MIMIR_CONNECTOR_PASSWORD` env ingestion, env-based kind inference for `auth` without config, the clap conflict between the two stdin flags, empty-stdin rejection (no `POST /connectors` after it), flag-beats-env and stdin-beats-env precedence, and the both-env-vars rejection for `auth`.
- **Docs.** `docs/cli.md`, `docs/connector-management.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, and `Mimir-Implementation-Context.md` updated with the new channels, precedence, and secret-hygiene guidance.
- Version bumped 0.112.0 → 0.113.0 (minor — backwards-compatible new feature).

## [0.112.0] — 2026-08-16

### AppState construction decomposed into per-subsystem init helpers (issue #265)

- **`mimir-server` `AppState` construction is no longer a monolith.** `state/builder.rs` now exposes `pub(super)` per-subsystem init helpers — `init_context_manager`, `init_tool_registry`, `init_knowledge_graph`, `init_job_queue`, `init_agent_runtime`, `init_scheduler`, and `init_connector_framework` — composed by `from_config_with_llm` in the same fixed startup order as before (context → tools → knowledge graph → job queue → agent runtime → scheduler → connector framework). `init_knowledge_graph` returns a small `KnowledgeGraphInit` struct carrying the shared knowledge graph, geocoder, resolved user entity id, and backup directory so later steps consume them without re-deriving state. The `AppState` field set and the public `from_config` / `from_config_with_llm` API are unchanged; no behaviour or startup order changed.
- **Tests.** `mimir-server/src/state/tests.rs` gains `init_knowledge_graph_resolves_user_entity_and_registers_kg_tools` (user-entity resolution, identity-fact seeding, KG tool registration, default geocoder, backup dir), `init_scheduler_registers_system_jobs` (knowledge-optimization, memory-condensation, pending-cleanup, and events-scan jobs registered), and `init_connector_framework_registers_mock_backend` (mock factory registered under `cfg(test)`).
- **Docs.** `docs/chat-server.md` (init-helper composition under Module Layout), `docs/wiki/what-works-now.md` (daemon-startup row; #265 removed from backlog), and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.111.3 → 0.112.0 (minor — backwards-compatible refactor).

## [0.111.3] — 2026-08-16

### Email and Photos connectors: failure-safe in-memory cursor advance (issue #332)

- **The Email and Photos connectors no longer advance their in-memory cursors inside `Connector::sync`.** The Calendar connector's #314 pattern (the persisted `connectors.sync_cursor` is the single source of truth and only advances on a fully successful cycle; the supervisor hands it back via `Connector::on_cycle_succeeded`) now covers all three backends. The Email connector's `run_sync` no longer writes `last_uid` after the IMAP fetch — its durable LLM-extraction retry ledger (issue #262) only covers LLM-layer failures inside `extract`, so a hard extract/insert/persist failure previously lost the staged mail until a restart; the next in-process cycle now re-fetches the failed window from the last confirmed cursor, in IDLE (push) mode skipping the IDLE wait to do so (the IDLE notification for the failed window will not re-fire), and re-fetches are deduped against the staged buffer so re-staged LLM retries are never duplicated across failed cycles. The Photos connector's scan/event passes return the computed `PhotosCursor` without adopting it, and a new `rescan_pending` flag makes the next in-process `sync` re-scan the watch directory from the last confirmed cursor when a previous cycle failed — required because the file watcher does not re-deliver consumed events. A cycle that fails after `sync` therefore re-processes its staged items on the next in-process cycle for all three connectors; restart recovery and manual full syncs remain available but are no longer required.
- **Tests.** `mimir-connectors/src/email/imap_tests.rs::failed_cycle_reprocesses_staged_mail_on_next_sync` pins the Email contract (a failed cycle re-fetches the same window without duplicating the staged buffer; the cursor is adopted only via `on_cycle_succeeded`, after which the next cycle is incremental), `idle_failed_cycle_resyncs_without_waiting_for_next_push` pins the IDLE-mode contract (the next cycle re-fetches immediately without an IDLE push), and the two cursor tests now assert the marker stays put until adoption. `mimir-connectors/src/photos/behaviour_tests.rs` gains `sync_reports_cursor_without_advancing_in_memory` and `failed_cycle_rescans_staged_photos_on_next_sync`; `mimir-connectors/tests/photos_connector.rs::failed_extract_cycle_reprocesses_staged_photos_on_next_cycle` drives a real Photos connector through the supervisor — the first automatic cycle fails at `extract` after staging the photo, the retry cycle re-scans from the last confirmed cursor, and the photo's fact lands in the KB with the cursor persisted.
- **Docs.** `docs/email-connector.md`, `docs/photos-connector.md` (new "Failure-safe adoption (#332)" section), `docs/connectors-framework.md`, `docs/wiki/email-connector.md`, `docs/wiki/photos-connector.md`, `docs/wiki/what-works-now.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.111.2 → 0.111.3 (patch — backwards-compatible bug fix).

## [0.111.2] — 2026-08-16

### Calendar connector: failure-safe in-memory sync-token advance (issue #314)

- **The Calendar connector no longer advances its in-memory `sync_token` inside `Connector::sync`.** The supervisor persisted `connectors.sync_cursor` is the single source of truth and only advances on a fully successful cycle; the new `Connector::on_cycle_succeeded(new_cursor)` trait method (default no-op) hands the persisted cursor back to the connector after the cycle's extraction, trashing, fact insertion, and cursor/durable-state persistence all succeeded. A cycle that fails after `sync` (extract error, trash error, hard `normalize_and_insert` error, or persist error) therefore leaves the in-memory marker at the last confirmed cursor, and the next in-process cycle re-syncs from it — the server re-reports the failed window's changed events and deletions, so no staged event or tombstone is permanently lost until a restart or manual full sync (the previous behaviour). Restart recovery and manual full syncs remain available, but are no longer required. Tombstone staging now dedupes by href so repeated re-syncs of a failed window cannot grow the pending deletion buffer.
- **Tests.** `mimir-connectors/tests/calendar_kb_tests.rs::failed_extract_cycle_reprocesses_staged_events_on_next_cycle` drives a real CalDAV connector through the supervisor: the first automatic cycle fails at `extract` after staging event A, a manual trigger re-syncs from the last confirmed cursor (no token) and event A's facts land in the KB, and the second trigger (the third cycle) syncs incrementally from the adopted token-1 (proving the cursor advanced only after success). `mimir-connectors/src/supervisor/cycle.rs::cycle_adopts_new_cursor_only_after_success` pins the supervisor contract — the connector is handed the new cursor only after a successful cycle, never after a failed one. The unit-level `incremental_sync_uses_persisted_sync_token` test now adopts the cursor between syncs the way the supervisor does.
- **Docs.** `docs/calendar-connector.md` (new "Failure-safe cursor adoption (#314)" section + sync-protocol/tombstone updates), `docs/connectors-framework.md` (trait + supervisor cycle), `docs/wiki/calendar-connector.md`, `docs/wiki/what-works-now.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.111.1 → 0.111.2 (patch — backwards-compatible bug fix).

## [0.111.1] — 2026-08-15

### Docs: connectors-framework.md ConnectorFactory signature synced with the landed ConnectorContext (issue #301)

- **`docs/connectors-framework.md` factory section now matches the code.** The `ConnectorFactory` API block documents `create(config, ctx)` with the shared-services `ConnectorContext` (optional `Geocoder`, `SecretStore`, user identity, `LlmBackend`), the `ConnectorRegistry` API listing includes `create_with_context` (the supervisor's construction path) alongside the config-only `create` convenience path, the `FnConnectorFactory` closure shape is `Fn(serde_json::Value, &ConnectorContext) -> Result<…>`, and the "Construction context (forward-looking)" note is replaced with the landed state (which callers pass which context). The startup-restore paragraph no longer claims the construction context is deferred.
- **Stale doc comments in `mimir-connectors` fixed.** The `ConnectorFactory` trait and `ConnectorContext` struct doc comments in `src/connector.rs` and the `FnConnectorFactory` doc comment in `src/registry.rs` described the pre-`ConnectorContext` factory signature ("will be extended when F8 / F10 land"); they now describe the landed signature and context fields.
- Version bumped 0.111.0 → 0.111.1 (patch — documentation-only).

## [0.111.0] — 2026-08-15

### Testing: shared wiremock token-endpoint mock for the PKCE suites (issue #298)

- **`mimir_connectors::test_utils::mount_token_endpoint(server, expected_calls)` is the single wiremock token-endpoint mock for the PKCE code exchange.** The PKCE flow's unit tests (`oauth::pkce`) inlined the same `POST /token` mock in four tests and the CLI connector tests (`mimir/src/connector/tests.rs`) carried a fifth private copy; the five mock definitions are replaced by the shared helper, which all six call sites (four PKCE flow tests and two CLI connector tests) now mount. The helper owns the canonical token-response shape (access token, token type, refresh token, expiry) and takes the per-test `expect(N)` count as a parameter, so the response shape can no longer drift between suites and a flow change cannot silently desynchronise the expected call count. The helper lives in the `test-utils` feature module (off by default) next to the #290 fake-browser doubles; `wiremock` became an optional dependency of `mimir-connectors` enabled by `test-utils` (the crate's own unit tests still get it from dev-dependencies), so production builds never compile it.
- **Tests.** A unit test mounts the helper and asserts the canonical response body; the existing PKCE flow tests (expect 0/1 call counts) and the two CLI PKCE tests exercise the helper through the real flow.
- **Docs.** `docs/e2e-testing.md`, `docs/oauth-client.md`, `docs/wiki/Testing-and-Benchmarks.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.110.2 → 0.111.0 (minor — backwards-compatible refactor).

## [0.110.2] — 2026-08-15

### Confidence: adjusted connector reliability scores now reach the connector pipeline (issue #292)

- **`confidence::connector_reliability` is the single confidence-resolution helper for every fact-insert path.** `normalize_and_insert` (the connector ingestion pipeline) previously computed confidence from `confidence::initial`'s hardcoded defaults, and `insert_facts_batch` passed `None` as the connector type, so `adjust_connector_reliability` had no effect on connector ingestion. Both paths now read the `connector_reliability` table: the pipeline resolves the score once per batch from the provenance's connector type (gated on both the instance id and the type), and the batch path resolves each connector instance to its registered type — validating a supplied `connector_type` and deriving an omitted one — then caches the reliability score per distinct resolved type (one query per type, cached across the batch, mirroring `insert_fact`'s `connector_instance_id` gate). `insert_fact`'s inline table read was refactored onto the same helper. Adjusted scores therefore reach connector extractions immediately; non-connector source types keep their existing `confidence::initial` defaults.
- **Tests.** Integration tests adjust the Calendar score and assert `normalize_and_insert` inserts at the adjusted score, and adjust the Photos score while leaving Calendar at its seed to prove `insert_facts_batch` resolves per-type table scores (0.85 / 0.90) instead of the 0.80 fallback. Regression tests cover an omitted `connector_type` deriving the registered instance's score, a supplied type mismatching the registered instance being rejected, and a `Provenance` carrying a type without an instance id falling back to the generic Connector default.
- **Docs.** `docs/Confidence-Model.md`, `docs/connectors-framework.md`, `VISION/03-Connectors/Technical-Design.md`, `Mimir-Implementation-Context.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.110.1 → 0.110.2 (patch — backwards-compatible bug fix).

## [0.110.1] — 2026-08-15

### CLI: `key=value` config pairs now express JSON arrays and objects (issue #289)

- **`parse_config_scalar` parses JSON arrays/objects.** A `key=value` value that starts with `[` or `{` is attempted as JSON — `auth.scopes=["https://mail.google.com/"]` reaches the merged config as an array instead of a plain string — and falls back to the string form when the JSON does not parse, so existing string values like `[unterminated` keep working. The OAuth PKCE flow's `auth.scopes` reader (`oauth_flow_config` in `mimir/src/connector/oauth.rs`) previously dropped a string-typed scopes value silently, producing an authorize URL with no scopes; array values from `key=value` pairs now reach the flow, and the `--config-json` workaround is no longer needed for array-typed keys.
- **Tests.** Unit tests cover scalar parsing of JSON arrays/objects (including empty `[]`/`{}`), the malformed-JSON string fallback, array/object values reaching the merged config through dotted keys, and `oauth_flow_config` receiving scopes supplied as a `key=value` pair.
- **Docs.** `docs/cli.md`, `docs/connector-management.md`, `docs/wiki/cli-commands.md` (including the `auth.scopes` example), and `docs/wiki/what-works-now.md` (open-items row removed) updated.
- Version bumped 0.110.0 → 0.110.1 (patch — backwards-compatible bug fix).

## [0.110.0] — 2026-08-15

### Email connector: iMIP CANCEL invites now trash the cancelled event's facts (issue #283)

- **CANCEL is no longer skipped.** A `text/calendar; method=CANCEL` iMIP part emits no facts; each cancelled VEVENT's namespaced reference (`imip:{uid}`) is buffered as a tombstone and reported via `Connector::extract_deletions`, and the supervisor trashes the facts this instance authored for that `raw_reference` through the shared #247 machinery (`KnowledgeGraph::forget_connector_facts_by_raw_reference` — recoverable for 30 days, idempotent, non-destructive until the cycle's trashing, insertion, and cursor persistence all succeed). A cancelled meeting therefore stops surfacing in "Upcoming" and its events-subsystem overlay is cascade-deleted with the facts. A CANCEL VEVENT without a `UID` cannot be mapped and is skipped; `SEQUENCE` is not consulted in V1 (the KB does not store the original sequence). A REQUEST and its CANCEL arriving in the same sync batch resolve to the CANCEL regardless of message order: after the message loop, `extract` drops every fact whose `raw_reference` matches a buffered tombstone, so the cancelled event is not re-inserted by the same cycle that trashed its prior facts. The tombstone buffer is part of the connector's durable state (persisted with the prose-retry ledger and restored at construction), so a restart between `extract` and the supervisor's deletion pass re-reports the removals instead of losing them. A handled CANCEL also counts as "read" for the extraction cascade gate, so the LLM layer never runs on cancellation prose.
- **iMIP facts are keyed by the namespaced VEVENT `UID`.** REQUEST/REPLY facts now carry the VEVENT `UID` namespaced as `imip:{uid}` as their `raw_reference` — the stable iMIP identity RFC 5546 requires every method (REQUEST → REPLY → CANCEL) to share — so a CANCEL maps 1:1 onto the facts the original invite authored. The `imip:` prefix keeps the sender-controlled iMIP identity space disjoint from the `{uid_validity}:{uid}` references the JSON-LD and LLM layers write, so a crafted `UID` can never address another layer's facts. A VEVENT without a `UID` (invalid per RFC 5545, tolerated by the lenient parser) falls back to the email's `UIDVALIDITY`-qualified IMAP UID in its own `imip-email:` namespace. The JSON-LD and LLM layers keep the email UID — they have no CANCEL lifecycle.
- **Legacy raw-reference boundary.** Facts authored before 0.110.0 carry the email's `UIDVALIDITY`-qualified IMAP UID as their `raw_reference`, so CANCEL tombstones cannot match them. The required cleanup is to remove each Email instance's pre-upgrade facts (`mimir connector forget <slug>` — recoverable from trash for 30 days), re-add and re-authenticate the Email connector (forget removes the connector row and credentials), and trigger a full re-sync so invites are re-authored with `imip:`-namespaced VEVENT-UID references — the same documented boundary the Calendar connector adopted in 0.103.0 (#247).
- **Tests.** Unit tests cover the CANCEL tombstone buffer (and the UID-less CANCEL no-op), the UID-less-REQUEST `imip-email:` fallback, the namespaced VEVENT-UID keying of REQUEST/REPLY clusters, a same-batch REQUEST+CANCEL resolving to the CANCEL regardless of message order (including a CANCEL staged before its REQUEST), a restart between `extract` and the deletion pass re-reporting the buffered tombstone from the durable state, and the cascade gate skipping the LLM layer on a CANCEL; a KB integration test stages a REQUEST then a CANCEL and proves the full lifecycle — the buffered reference trashes all four cluster facts, the events-subsystem overlay is cascade-deleted, the removal is acknowledged, and a re-report is an idempotent no-op.
- **Docs.** `docs/email-connector.md` (C6 section: CANCEL lifecycle, fact keying, legacy boundary; testing section), `docs/wiki/email-connector.md`, `docs/connectors-framework.md` (tombstone trait surface now names the Email connector), `docs/wiki/what-works-now.md`, and `README.md` updated.
- Version bumped 0.109.0 → 0.110.0 (minor — bug fix plus the Email iMIP raw-reference scheme change, a breaking data-semantics change acceptable per the project's internal-API policy; mirrors the 0.103.0 Calendar precedent).

## [0.109.0] — 2026-08-15

### DRY: shared forget fact-filter SQL builder (issue #267)

- **`push_forget_filters` is the single home for the `ForgetFilters` WHERE clauses.** `mimir-knowledge/src/forget/trash.rs` previously copy-pasted every filter clause (`fact_id`, `predicate`, `subject`, `entity`, `source`, `from`, `to` — including the connector-source subquery) between `query_matching_fact_ids` (the bulk-forget id list) and `has_sensitive_match` (the sensitive-predicate safeguard); both now push through the shared helper, so a new filter field is added in exactly one place and the two queries cannot drift. Pure refactor — no behaviour change.
- **Tests.** A SQL-shape unit test locks the exact clauses both queries emit, and a behavioural test runs both queries against a real DB across every filter field (including the connector-slug and source-type `source` paths and a pinned `created_at` window) asserting the id list and sensitive flag agree.
- **Docs.** `docs/fact-management.md` (new "Bulk forget matching" section, stale `forget.rs` → `forget/` module reference) and `docs/wiki/what-works-now.md` (Calendar row and DRY backlog no longer list #267) updated.
- Version bumped 0.108.0 → 0.109.0 (minor — internal refactor; no public API or behaviour change).

## [0.108.0] — 2026-08-15

### Feature: connector catalog — discover registered types/backends (issue #271)

- **`GET /connectors/catalog` is the authoritative discovery surface.** It returns every registered `(connector_type, backend)` pair from the daemon's `ConnectorRegistry` (`ConnectorCatalogResponse` with sorted `ConnectorCatalogEntry { connector_type, backend }` entries), populated at request time so feature-gated registrations (`photos`/`calendar`/`gmail`, plus the `mock-connector` test backend) are reflected automatically. The registry gained `ConnectorRegistry::pairs()`, which sorts by type then backend (wire-string form) so output never depends on `HashMap` iteration order. The static path wins over `GET /connectors/{id}`.
- **`mimir connector catalog` renders the catalog.** `mimir-client::connector_catalog()` plumbs the route; the CLI shows a type/backend table (or `--json`), so users never have to guess backend strings. The `--backend` help text now points at the command.
- **`mimir connector add` pre-flights the pair before prompting.** A typo'd or feature-disabled `(connector_type, backend)` now fails immediately with the supported set for that type (or the full supported-pairs list for an unknown type) instead of failing at `POST /connectors` with a 400 after the credential prompt or the whole interactive OAuth PKCE flow. The daemon stays authoritative — the POST still validates — so this is a UX fast-fail, not a security boundary.
- **Tests.** Registry unit test (sorted pairs), server route integration test, CLI wiremock tests (catalog table/JSON round-trip, pre-flight rejection with no POST), and a daemon E2E test asserting the catalog advertises exactly the feature-gated registrations (`photos/local`, `calendar/caldav`, `gmail/imap`, `gmail/test`).
- **Docs.** `docs/connector-management.md` (route table + wire types), `docs/cli.md`, `docs/wiki/cli-commands.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, and `VISION/03-Connectors/Technical-Design.md` (new "Registry Discovery" section) updated.
- Version bumped 0.107.1 → 0.108.0 (minor — new feature).

## [0.107.1] — 2026-08-14

### Robustness: per-connector lifecycle serialisation + graceful runner stop (issue #266)

- **Regression tests lock the serialisation invariant.** A burst of concurrent `start` / `resume` calls on one instance leaves exactly one live runner with no overlapping syncs (`concurrent_start_resume_leaves_single_runner`, multi-threaded, sync-recorder-verified), and `pause` / `start` demonstrably queue on the same per-connector lifecycle lock (`pause_and_start_share_the_per_connector_lifecycle_lock`). The per-connector `lifecycle_lock` itself landed with #268; these tests guard it against regression.
- **`pause` now holds the per-connector lifecycle lock** across the whole stop → status-write sequence, like `start` / `resume` and the daemon's forget cascade. A concurrent `pause` + `start` can no longer leave a `Paused` row with a live runner that keeps syncing.
- **The `DELETE /connectors/{id}` route holds the same lifecycle lock** across its whole stop → secret-delete → row-delete sequence, so a concurrent `resume` can never re-spawn a runner for a row that is about to disappear.
- **`stop` is now graceful and cycle-complete.** `ConnectorSupervisor::stop` signals the runner over a per-runner `watch` channel and awaits its termination; the runner aborts and awaits its in-flight cycle sub-task before exiting, so a stopped connector can no longer keep syncing or writing facts after `stop` returns, and a re-spawn's first cycle never overlaps the previous runner's last cycle (previously the aborted runner detached its in-flight cycle, which kept running — reproduced as `max_concurrent == 6` under an 8-way start burst). A `Drop` guard on the cycle's `AbortHandle` covers the abort fallback path (`shutdown()`).
- **`stop` preempts an in-flight auth handshake.** The runner's initial `authenticate()` is selected against the stop/shutdown signals, so a slow or hung handshake (e.g. an unreachable IMAP/CalDAV server) can no longer block `stop` — and with it `pause` / `DELETE` / re-spawn — for the whole network timeout.
- **`shutdown` now awaits every in-flight cycle.** `ConnectorSupervisor::shutdown` signals each runner and awaits its graceful exit (the runner aborts and awaits its cycle before returning) instead of aborting immediately; stragglers are aborted only after a grace period, and their cycle `JoinHandle`s stay registered in a cycle registry that `shutdown` drains and awaits afterwards, so no cycle task can outlive `shutdown` and write facts after teardown (regression test `shutdown_awaits_an_in_flight_cycle`).
- **Test/doc hygiene (PR #321 review).** The leak-window test now uses a 5 s sync delay against the 300 ms observation window so the surviving runner's sync is provably still in flight when the window closes; the stale "Supervisor start/resume race" pending entry for #266 was removed from `docs/wiki/what-works-now.md`, and the shutdown semantics in `docs/connector-management.md` / `docs/connectors-framework.md` were updated.
- **Docs.** `docs/connectors-framework.md` (new "Lifecycle control" section), `docs/connector-management.md` (stop/pause/resume semantics), `docs/wiki/connectors.md`, and `docs/wiki/what-works-now.md` updated; issue #305's pause/resume flake is closed by the graceful stop.
- Version bumped 0.107.0 → 0.107.1 (patch — backwards-compatible robustness fix).

## [0.107.0] — 2026-08-14

### Robustness: explicit enum→wire-string conversion (issue #264)

- **`as_str()` is now the single source of truth for wire strings.** `ConnectorType`, `ConnectorStatus`, `ConnectorAuthState` (in `mimir-knowledge`) and `JobPriority` (in `mimir-core`) gained explicit `as_str()` methods; `JobRunStatus::as_str()` is now public. The route layer (`mimir-server`) no longer derives wire strings from `format!("{:?}").to_lowercase()` — every call site in the connector routes, the KB optimization status/run-now handlers, the memory condensation handler, and the `CONNECTOR_NOT_RUNNING` error detail now calls `as_str()`, so a variant rename or a custom `Debug` impl can no longer change the API output silently.
- **Deliberate wire-output change:** `JobRunStatus::TimedOut` now serialises as `"timed_out"` (underscored, matching the DB representation and `JobRunStatus::from_str`) instead of the `Debug`-derived `"timedout"`. All other wire strings are byte-identical to before.
- **Why `as_str()` over serde `rename_all`:** `mimir-api-types` is deliberately decoupled from `mimir-knowledge`, so serialising the enums through serde in typed route bodies would break that boundary; the explicit methods keep the decoupling and make the input (`parse_connector_type`) and output directions symmetric.
- **Tests.** New unit tests lock the wire contract for all five enums (every variant, including the `timed_out` spelling); the existing connector-route and optimization-route integration tests pass unchanged.
- **Docs.** `docs/connector-management.md` (wire-types note), `docs/job-queue.md` (new "Wire strings" section), and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.106.2 → 0.107.0 (minor — refactor; the `TimedOut` wire spelling is an internal-API change acceptable per the project's internal-API policy).

## [0.106.2] — 2026-08-14

### PR #318 review: atomic retry-ledger persistence, aggregate ledger cap, test/doc hygiene

- **Atomic cursor + durable-state persistence.** The supervisor now persists the sync cursor and the connector's durable state (the Email retry ledger) in one transaction via the new `KnowledgeGraph::update_sync_progress_and_durable_state` (`None`-means-unchanged for both fields), so a crash between the two writes can no longer advance the cursor without its retry record — a restart previously skipped the failed message because the cursor advanced without its ledger entry. `durable_state_persisted` is acknowledged only after the combined commit.
- **Aggregate persisted-ledger cap.** `durable_json` now sheds raw payloads beyond `MAX_PERSISTED_PENDING_PAYLOADS` (32) per snapshot, so a mailbox-wide LLM outage cannot grow `connectors.durable_state` with one base64 payload per failing message; entries beyond the cap still retry in-process with the full payload (a restart drops them, matching the oversized-payload behaviour).
- **Hygiene.** Test fixtures consolidated (`llm_tool_response` reuses `llm_tool_message`, the re-stage test reuses `prose_email()`), the three `instantiate_*` tests share a `capturing_supervisor` helper, and the policy-section wording in `docs/email-connector.md` is fixed.
- **Tests.** New ledger-policy test for the persisted-payload cap and a knowledge-graph test locking the atomic combined persist (advance/unchanged combinations + missing-row error).
- **Docs.** `docs/connectors-framework.md` (atomic persistence protocol), `docs/email-connector.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.106.1 → 0.106.2 (patch — backwards-compatible bug fix and documentation).

## [0.106.1] — 2026-08-14

### Email connector: durable retry / terminal-failure policy for LLM prose extraction (issue #262)

- **Bounded, restart-safe retries.** The Email connector's LLM prose-extraction retry is no longer an unbounded, in-memory re-stage loop. A new per-instance retry ledger (`mimir-connectors/src/email/llm/retry.rs`) bounds each message to `llm_extraction_max_attempts` attempts (default 3, configurable, minimum 1) with exponential cycle backoff (1, 2, 4, … capped at 8), and records a **terminal failure** with the last error once the budget is exhausted — the message stops consuming LLM calls and is no longer re-staged, while deterministic-layer facts are still never blocked by an LLM failure.
- **Durability across restarts.** The ledger (pending items with their raw RFC 822 bytes base64-encoded, plus capped terminal-failure records) is persisted by the supervisor after each successful extraction cycle via a new generic `KnowledgeGraph::update_durable_state` (`connectors.durable_state` column, migration 049) and re-injected at connector construction as `__durable_state`, so a `mimir stop` / restart resumes the bounded retry instead of dropping the message. `Connector` gains defaulted `durable_state()` / `durable_state_persisted()` hooks; the supervisor persists the state after `extract()` alongside the sync cursor and acknowledges the write only after it succeeds, so a failed database write is re-attempted next cycle instead of silently losing the ledger. Persisted raw payloads are capped at 512 KiB (larger messages still retry in-process; a restart drops them) so the durable column cannot grow without bound.
- **Surfacing.** A non-empty terminal-failure backlog makes the Email connector's `health()` report `Degraded` (reachable, but repeated per-message failures) instead of `Online`; `forget` clears the ledger.
- **Tests.** 14 new unit tests for the ledger policy (including write-through persist acknowledgement, overlap sanitisation of restored ledgers, and the persisted-raw size cap), 4 new cascade tests (bounded-failure → terminal, success-within-budget settles, `max_attempts: 1` fails terminal immediately, restart-resume from a captured `__durable_state` without an IMAP re-fetch), a supervisor-cycle persistence + acknowledgement test, an instantiate injection test, a config default/override test, and a knowledge-graph `update_durable_state` facade test.
- **Docs.** `docs/email-connector.md` (new "Failure and retry policy" section + config), `docs/connectors-framework.md` (trait hook, supervisor persistence, schema, facade list), `docs/knowledge-graph-schema.md` (migration 049), `docs/wiki/email-connector.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.106.0 → 0.106.1 (patch — backwards-compatible bug-fix/robustness; the new column and trait default are additive).

## [0.106.0] — 2026-08-14

### DRY: shared LLM tool-output parsing across conversational and connector extraction (issue #259)

- **One parser owns the tool-call + fence-fallback dance.** `mimir-core::llm::parse_tool_output<T>` (new `mimir-core/src/llm/tool_output.rs`) now handles the three-step parse — first `tool_calls` entry's `function.arguments`, else ```fence```-stripped `content`, else error — once, with a shared `ToolOutputParseError` enum that callers map onto their own error types. `mimir-knowledge::extract::parse::parse_remember_output` (conversational `remember` tool) and `mimir-connectors::email::llm::parse::parse_output` (Email C7 / #201 `extract_email_facts` tool) both delegate to it; the email-specific exactly-one-call and tool-name guards are preserved through the shared parser's expected-tool-name check, and the conversational bare-`Vec<ExtractedFact>` fallback is preserved on top of the shared parser. The duplicated `strip_code_fence` helper is gone.
- **Tool schemas are built once.** `remember_tool_schema()` and `email_extraction_tool_schema()` are now `LazyLock`-cached statics returning `&'static serde_json::Value` (the schemas are static — there is no per-call input), so a long email sync no longer rebuilds the identical schema tree per extraction. `remember_tool_schema`'s return type changes from owned `Value` to `&'static Value` (internal API; callers clone where an owned value is needed).
- **Tests:** 11 new unit tests for `parse_tool_output` (tool-call, expected-name guards, empty-list, invalid arguments, plain/fenced content fallback, empty content, invalid-JSON text carry) in `mimir-core`, plus 4 new unit tests locking `parse_remember_output`'s wrapper / fenced-wrapper / bare-array fallback behaviour in `mimir-knowledge`. The existing email `parse_output` guard tests and the conversational text-fallback integration tests pass unchanged.
- **Docs:** `docs/llm-backend.md` (new "Shared Tool-Output Parsing" section), `docs/fact-extraction-pipeline.md`, `docs/email-connector.md`, and `docs/wiki/what-works-now.md` updated.
- Version bumped 0.105.0 → 0.106.0 (minor — internal refactor; no behaviour change, `remember_tool_schema`'s return type is an internal-API break acceptable per the project's internal-API policy).

## [0.105.0] — 2026-08-14

### DRY: shared `connector_fact` constructor for connector facts (issue #255)

- **One helper owns the connector defaults.** The Photos connector built `NormalizedFact` struct literals in three places (`place_fact`, `visited_fact`, `took_photo_fact`), each repeating the same connector-level boilerplate (`source_type: Connector`, non-sensitive, non-correction, no category ids, no user action). A new always-compiled `mimir_connectors::fact::connector_fact` — generalized from the Calendar/Email `vevent_fact` helper, renamed for its wider use — now owns those defaults once, with the per-shape fields (subject, relationship, object, entity-ness, temporal bounds, recurrence, raw reference, extraction method, event-type hint, location overlay) as arguments.
- **All three backends funnel through it.** The iCal VEVENT cluster (`vevent_to_facts`), the Email JSON-LD extractor (`jsonld_fact` wrapper), and all three Photos fact shapes call `connector_fact`; the `vevent_fact` name (misleading once JSON-LD reused it) is gone.
- **No behaviour change.** Photos facts keep `extraction_method: None` (inheriting the supervisor's `StructuredParse` batch provenance), Calendar/Email facts keep the explicit `Some(StructuredParse)` per-fact override, and every fact field is identical to before. The helper lives in an always-compiled module, so a Photos-only build (`--no-default-features --features photos`) still compiles.
- **Tests:** a new contract test pins the helper's fixed defaults and the override params (entity vs literal object, location overlay, extraction method), so a future producer cannot silently reintroduce drifted defaults.
- **Docs:** `docs/photos-connector.md`, `docs/connectors-framework.md`, `docs/refactoring-module-split.md`, and `docs/wiki/what-works-now.md` updated for the shared constructor.
- Version bumped 0.104.0 → 0.105.0 (minor — internal refactor; no public API or behaviour change).

## [0.104.0] — 2026-08-14

### Photos connector: coords-only fallback authors a real-world `visited` fact, not a file-path object (issue #250)

- **Facts-vs-provenance for coords-only photos.** When a photo has GPS but no place name resolves (no geocoder, a genuine no-match, or a transient geocode error), `RawPhoto::to_fact` now authors `owner visited <coords-label>` instead of the C1 `owner took_photo <rel_path>` shape — the primary fact expresses the real-world event ("you visited <place> at <time>") consistent with the email connector's facts-vs-provenance principle (#200), and the photo's watch-dir-relative path remains only as `raw_reference` provenance. No data is lost.
- **Stable per-spot label.** The object is a millidegree-rounded coordinate label (e.g. `"46.500, 7.500"`) derived from the same GPS bucket as the reverse-geocode cache key (~111 m), so photos at the same spot author the same object and corroborate into one `visited` fact per spot — mirroring the per-locality merge of `took_photo_at` facts. `visited` is an existing canonical predicate; literal objects do not participate in the transitivity inference rule.
- **No-GPS photos unchanged.** A photo without GPS evidences no real-world visit, so it keeps the literal `took_photo <rel_path>` timestamp-only record (useful for photo-count queries) with no location overlay. The `entity_locations` `Visited` overlay for coords-only photos is unchanged, so location-history and proximity-query behaviour is preserved.
- **Tests:** unit, behaviour, and supervisor-level integration tests updated to the `visited` shape (including the landed-fact predicate and `raw_reference` provenance assertions), plus a same-bucket label-equality test and a no-GPS `took_photo` retention test.
- **Docs:** `docs/photos-connector.md`, `docs/wiki/photos-connector.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, and `README.md` updated for the new fallback shape.
- Version bumped 0.103.1 → 0.104.0 (minor — the connector's emitted fact data semantics change: coords-only photos now author `visited <coords-label>` facts; existing `took_photo <path>` facts in the KB are untouched, and a connector forget + full re-sync converts them to the new shape).

## [0.103.1] — 2026-08-14

### PR #313 review fixes: tombstone retention, multi-source preservation, and legacy Calendar raw-reference upgrade note

- **Tombstones survive failed cycles.** `Connector::extract_deletions()` is now non-destructive (it reports the pending tombstone buffer instead of draining it) and the supervisor calls the new `Connector::acknowledge_deletions()` only after the cycle's trashing, fact insertion, and cursor persistence all succeeded — so a transient deletion-processing failure re-reports the removals on the next in-process cycle instead of losing them forever, and a restart still resumes from the un-persisted cursor (PR #313 review). The in-memory-token gap for a failed cycle's *changed-event* buffer remains tracked as #314.
- **Facts corroborated by another source are preserved.** `KnowledgeGraph::forget_connector_facts_by_raw_reference` now removes only the matching `sources` rows (same transaction) and trashes a fact only when no sources remain, so a tombstone from one connector instance can no longer delete a fact another connector or a non-connector source still supports. The returned count reflects facts actually trashed.
- **Legacy Calendar raw-reference compatibility boundary.** Pre-0.103.0 Calendar facts carry the VEVENT `UID` as their `raw_reference`, so href-based tombstones cannot match them; the required cleanup is to remove each Calendar instance's pre-upgrade facts (connector-forget, recoverable from trash) and trigger a full re-sync so events are re-authored with href references — documented in `docs/calendar-connector.md`, `docs/wiki/calendar-connector.md`, and the 0.103.0 entry below.
- **Tests:** supervisor-level transient deletion-failure → next-cycle retry (fact removed), mock retention/acknowledgement semantics, and KB multi-source preservation; existing tombstone E2E and idempotency tests updated to the acknowledgement flow.
- **Docs:** `docs/calendar-connector.md`, `docs/connectors-framework.md`, `docs/mock-connector.md`, `docs/wiki/calendar-connector.md`, `docs/wiki/mock-connector.md` updated for the retention/acknowledgement flow and the legacy-reference upgrade note.
- Version bumped 0.103.0 → 0.103.1 (patch — backwards-compatible bug fixes for the 0.103.0 tombstone path; no API removals, `acknowledge_deletions` is a defaulted trait method).

## [0.103.0] — 2026-08-14

### Calendar connector: propagate server-side deletions (tombstones) to the KB fact lifecycle (issue #247)

- **Tombstone drain on the connector trait.** New `Connector::extract_deletions()` (default empty) lets a connector report the `raw_reference`s its service removed since the last cycle; the supervisor calls it every cycle after `extract()` and trashes the matching facts before inserting that cycle's insertions (so a raw item deleted and re-created within one window ends up represented by the fresh facts).
- **Instance-scoped KB trashing.** New `KnowledgeGraph::forget_connector_facts_by_raw_reference(instance_id, raw_references, changed_by)` trashes exactly the facts that instance authored for those `sources.raw_reference` values through the shared trash machinery (30-day recovery, inferred-child cascade, audit). Idempotent: a tombstone reported twice trashes nothing the second time (mirroring `delete_event`'s 404-is-success semantics), and another instance's facts with the same raw reference are never touched. The `events.fact_id` FK cascade removes the events-subsystem overlay with the fact, so a deleted event stops surfacing in "Upcoming" and can never advance as an orphan.
- **CalDAV surface.** `CalendarConnector::stage` moves `sync-collection` `deleted` hrefs into a tombstone buffer (instead of logging them), `extract_deletions()` reports it, and the extractor now authors each event fact's `raw_reference` as the resource **href** (the server-side item id) so a tombstone maps 1:1 onto the facts — the VEVENT `UID` remains only the Event-entity name fallback. Deletions ride the existing sync-token incremental window, so no new cursor is needed; a trash failure aborts the cycle before the cursor persists, so a restart resumes from the old sync-token and re-reports the deletion (an in-process retry does not re-fetch — the in-memory token has already advanced; tracked as #314).
- **Mock harness.** `MockConnector` gains a `deletions` config knob (staged by every `sync`, drained by `extract_deletions`) so the deletion path is testable without a real service.
- **Tests:** KB-level raw-reference trashing (instance/raw-reference scoping, idempotency, overlay cascade), supervisor-level mock tombstone round trip, and a CalDAV E2E test (sync event → Upcoming → server-side deletion → facts trashed, Upcoming hides it, overlay gone).
- **Docs:** `docs/calendar-connector.md` (new "Server-side deletions" section), `docs/connectors-framework.md` (trait surface), `docs/fact-management.md` (KB API), `docs/mock-connector.md` + `docs/wiki/mock-connector.md` (mock knob), `docs/wiki/calendar-connector.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`.
- Version bumped 0.102.2 → 0.103.0 (minor — backwards-compatible new API surface: defaulted `Connector::extract_deletions` + new `KnowledgeGraph` method; the Calendar connector's raw-reference scheme changes to the resource href, a breaking data-semantics change acceptable per the project's internal-API policy). **Upgrade note:** facts authored before 0.103.0 carry the VEVENT `UID` as their `raw_reference`, so href-based tombstones cannot match them; after upgrading, remove each Calendar instance's pre-upgrade facts (connector-forget) and trigger a full re-sync so events are re-authored with href references — otherwise a pre-upgrade deleted event can keep surfacing in "Upcoming".

## [0.102.2] — 2026-08-14

### Photos connector: facts authored against the canonical user identity (issue #246)

- **Canonical identity wins.** `PhotosConnector` now reads `ConnectorContext::user_identity` (the `config.toml` `[identity] name`, injected by the daemon) at construction, and `extract()` authors `took_photo_at` / `took_photo` facts against it — so photo-derived facts resolve to the same `Person` entity the daemon resolves as `user_entity_id` and surface in user-scoped memory sections, instead of a disconnected per-instance `owner_name`.
- **`owner_name` demoted to fallback.** The per-instance `owner_name` config field (defaulting to the connector slug) is now used only when no identity is injected, so a library without a configured `[identity] name` still produces facts (unlike the Calendar connector, which skips its primary user fact when no identity is injected).
- **Factory wiring.** `PhotosConnectorFactory::create` forwards `ctx.user_identity` into the connector (as `CalendarConnectorFactory` does), so the daemon's configured identity reaches photo facts end-to-end.
- **Tests:** subject-precedence (identity over `owner_name`), `None`-identity fallback, and a factory-level end-to-end test (sync → extract) proving the injected identity is used.
- **Docs:** `docs/photos-connector.md` (new "User identity" section, subject/config/API updates), `docs/wiki/photos-connector.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, and `docs/calendar-connector.md` updated.
- Version bumped 0.102.1 → 0.102.2 (patch — backwards-compatible bug fix; non-blank `owner_name` values retain the existing fallback behaviour when no identity is configured, while blank or whitespace-only values fall back to the connector slug).

## [0.102.1] — 2026-08-14

### Phase 3 deps ledger reconciled with the MSRV-capped icalendar resolution (issue #239)

- **Ledger pins the resolved version.** `VISION/09-Roadmap/Phase-3-Plan.md` §4 now records `icalendar` as 0.17.6 — the latest release compatible with the workspace MSRV 1.85 (0.17.7+ requires Rust 1.88) — instead of the aspirational 0.17.12 that Cargo never resolves under the pinned toolchain. The `calendar` and `gmail` features keep declaring `icalendar = "0.17"`; the declaration is unchanged, only the ledger now documents the actual resolution with the MSRV constraint and a revisit trigger (a feature only available in 0.17.7+).
- **Ledger rows for `async-imap` and `mail-parser` corrected** to the versions actually declared in `mimir-connectors/Cargo.toml` (0.11.3 and 0.11.5 respectively) while reconciling the table against `Cargo.lock`.
- **Docs:** `mimir-connectors/Cargo.toml` dependency comment and `docs/calendar-connector.md` dependency table now state the MSRV cap instead of pointing at a follow-up issue; `docs/wiki/what-works-now.md` removes the #239 backlog row (and refreshes its stale version header).
- Version bumped 0.102.0 → 0.102.1 (patch — backwards-compatible documentation/maintenance fix; no code or dependency changes).

## [0.102.0] — 2026-08-14

### Refactor: entity-location queries move to `queries::location` (issue #231)

- **New `queries::location` module.** The `entity_locations` / `pending_location_meta` query functions (`insert_location`, `upsert_location`, `get_locations`, `update_location`, `close_prior_open_locations_in_tx`, `ensure_place_coordinates`, the pending-meta helpers, and `find_nearby` in `queries::location::nearby`) moved out of `queries::entity` into a dedicated top-level module, mirroring the per-table layout (`queries::event`, `queries::source`, …). `queries::entity` now owns only the `entities` table (CRUD, dedup, names, predicates).
- **Pure move, no behaviour change.** The `KnowledgeGraph` facade, the normalize/confirm pipelines, and the integration tests now reference `queries::location::…`; the public `KnowledgeGraph` API is unchanged.
- **Breaking (internal API): import-path migration.** The lower-level query paths moved from `queries::entity` to `queries::location`: `queries::entity::insert_location` → `queries::location::insert_location`, and likewise for `upsert_location`, `get_locations`, `update_location`, `close_prior_open_locations_in_tx`, `ensure_place_coordinates`, `insert_location_in_tx`, the pending-meta helpers (`insert_pending_location_meta`, `insert_pending_location_meta_in_tx`, `get_pending_location_meta`, `delete_pending_location_meta`), the `PendingLocationMeta` type, and `queries::entity::find_nearby` → `queries::location::find_nearby` (re-exported from `queries::location::nearby`). Direct callers of these paths must update their imports; the public `KnowledgeGraph` facade is unaffected.
- **Docs:** `docs/entity-locations.md`, `docs/photos-connector.md`, and `docs/refactoring-module-split.md` updated for the new module path.
- Version bumped 0.101.9 → 0.102.0 (minor — internal refactor; breaking only for direct importers of the lower-level `queries::entity` query module, which is not a public-facing interface).

## [0.101.7] — 2026-08-13

### Entity-location dedup hardened (PR #308 review)

- **Facade upserts serialised with the write lock.** `KnowledgeGraph::upsert_location` now holds the shared knowledge-graph write lock across the re-statement candidate lookup and the write, so two concurrent facade calls cannot both read a stale no-match and insert duplicate rows (or close each other's row). A concurrent facade-upsert test covers identical locations.
- **Start-less re-statements keep an unbounded start.** The interval-union merge previously pinned `valid_from` to the bounded statement's start when merging e.g. `2020-present` with a same-place claim of "until 2023" (no start); the union now stays unbounded (`None`) whenever either statement has no start, matching the overlap query's unbounded semantics. A test covers the bounded + start-unbounded union.
- **Docs:** the dedup contract (0.1 km coordinate tolerance, shared-attribute veto, overlapping-period-only merge) is now stated consistently in `docs/wiki/entity-locations.md`, `Mimir-Implementation-Context.md`, `README.md`, and `docs/wiki/what-works-now.md`; `docs/entity-locations.md` updated for the merge and locking rules.
- Version bumped 0.101.6 → 0.101.7 (patch — backwards-compatible bug fixes).

## [0.101.6] — 2026-08-13

### Entity-location re-statement deduplication (issue #228)

- **Same-place re-statements no longer create duplicate location rows.** `KnowledgeGraph::upsert_location` previously treated every new statement of the same `entity_id` + `location_type` as a move: a re-stated home (same address or coordinates, a new `valid_from`) closed the prior open-ended row and inserted a duplicate with identical shape — two rows for one continuous home. The upsert now detects a re-statement (same place, overlapping period) and folds it into the earliest matching row instead: bounds merge as an interval union (earliest `valid_from`, latest `valid_until`; an open-ended side stays open, so a same-place re-statement never closes an open "currently lives there" row), and missing shape fields (`address` / `latitude` / `longitude` / `timezone`) are filled from the re-statement. Same-place identity requires agreement on every shared attribute — different addresses, or coordinates more than 0.1 km apart, still take the move path, and disjoint periods of the same place stay distinct rows.
- **Tests:** a twelve-case dedup matrix in `mimir-knowledge/tests/entity_locations_test.rs` (open / timeless / identical-bounded re-statements, backward bounds extension, missing-geo-half fill, coords-only within-radius merge, the same-address-far-coords veto and coords-only beyond-radius distinctness rules, disjoint periods, bounded-does-not-close-open, different-address still supersedes, and an end-to-end corroborated re-statement through `normalize_and_insert`).
- **Docs:** `docs/entity-locations.md` gains a "Re-statement deduplication" section describing the identity, overlap, and merge rules; `docs/wiki/entity-locations.md` explains the behaviour in user-facing terms; `docs/wiki/what-works-now.md` marks #228 resolved.
- Version bumped 0.101.5 → 0.101.6 (patch — backwards-compatible bug fix).

## [0.101.5] — 2026-08-13

### Sensitive-fact location overlay hardened (PR #307 review)

- **Atomic pending-fact + location-shape persistence.** The `pending_location_meta` insert now happens in the same transaction as the pending-fact insert (`insert_sensitive_fact`), so a confirmable fact can never exist without the shape confirmation needs to rebuild its `entity_locations` row; if either write fails, both roll back and the fact is reported as an error instead of being left confirmable without its location payload.
- **Overlay meta consumed only on success.** `apply_location_overlay` now reports whether the `entity_locations` upsert succeeded, and `confirm_fact` deletes `pending_location_meta` only after a successful write — a failed write retains the shape (with a warning) so the overlay can be retried instead of losing the only location payload.
- **Tests:** the confirm-path tests now supply a bounded `valid_until` and assert the confirmed location row preserves both temporal bounds (`mimir-knowledge/tests/entity_locations_test.rs`, `extract/confirm_tests.rs`).
- **Docs:** `docs/entity-locations.md`, `docs/pending-fact-confirmation.md`, and `docs/wiki/entity-locations.md` updated for the atomic persistence and retention-on-failure behaviour.
- Version bumped 0.101.4 → 0.101.5 (patch — backwards-compatible bug fixes).

## [0.101.4] — 2026-08-13

### Sensitive-fact location overlay rebuilt on confirmation (issue #226)

- **Sensitive "where" facts keep their structured geo data across the confirmation boundary.** The entity-locations overlay was only applied on the non-sensitive (inserted) path; a sensitive location fact landed as `pending_confirmation` and `confirm_fact` never re-derived it, so a confirmed sensitive location fact lost its `entity_locations` row entirely. The sensitive path in `normalize::process_normalized_fact` now persists the `NormalizedLocation` shape into a new `pending_location_meta` table (migration `048`, the location analogue of `pending_event_meta`), and `extract::confirm_fact` rebuilds the overlay on confirmation — re-running the same geocode-fill + `upsert_location` with the confirmed fact's id and temporal bounds, then consuming the meta row. Rejecting the pending fact hard-deletes it, so `ON DELETE CASCADE` removes the meta row automatically and no orphan location row can be left behind.
- **Tests:** integration coverage in `mimir-knowledge/tests/entity_locations_test.rs` (confirm produces a geocoded row with temporal bounds + `source_fact_id`; reject leaves nothing) and a conversational-path unit test in `extract/confirm_tests.rs`.
- **Docs:** `docs/entity-locations.md` (pending-path section), `docs/pending-fact-confirmation.md`, `docs/knowledge-graph-schema.md` (new table + migration list 045–048), `docs/wiki/entity-locations.md`, and `docs/wiki/what-works-now.md`.
- **API addition:** `LocationType` now implements `TryFrom<i16>` (matching the other `#[repr(i16)]` enums in `models/enums.rs`), so raw lookup ids convert with a fallible typed conversion instead of a manual match.
- **Public API:** `queries::entity::{PendingLocationMeta, insert_pending_location_meta, get_pending_location_meta, delete_pending_location_meta}` — the `pending_location_meta` row model and its read/write/delete queries, mirroring the `queries::event` pending-event-meta API.
- Version bumped 0.101.3 → 0.101.4 (patch — backwards-compatible bug fix).

## [0.101.3] — 2026-08-13

### Location-type spec drift fixed (issue #224)

- **`location_types` taxonomy corrected to the seeded source of truth.** The #65 issue body and `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` listed `Previous(3) / Frequent(4) / EventLocation(5)`, but migration `001` and `models::enums::LocationType` seed `Visited(3) / Origin(4) / Current(5)` (plus `Geographic(6)` from migration `046`). The #65 body and the roadmap deliverable list now match the database and Rust enum, `docs/entity-locations.md` drops the obsolete drift note, `docs/knowledge-graph-schema.md` corrects the `location_types` row count to 6, and `docs/wiki/what-works-now.md` removes the resolved #222/#224 items from the stale-docs row.
- Version bumped 0.101.2 → 0.101.3 (patch — backwards-compatible documentation update).

## [0.101.2] — 2026-08-13

### DRY: RateLimitConfig::nominatim() delegates to Default (issue #223)

- **Single source of truth for the conservative rate-limit preset.** `RateLimitConfig::nominatim()` in `mimir-connectors/src/rate_limit/config.rs` was byte-identical to the `Default` impl (1 req/s, burst 1, no daily quota, exponential backoff), so the two could silently drift apart. The preset now delegates to `Self::default()` and keeps its named constructor for call-site readability; a regression test asserts `nominatim() == default()` so any future divergence fails CI.
- **Docs:** `docs/connector-rate-limiting.md` (Presets section) notes the preset is the default config, and `docs/wiki/what-works-now.md` drops the resolved #223 item from the Geocoder row.
- Version bumped 0.101.1 → 0.101.2 (patch — backwards-compatible refactor, no behaviour change).

## [0.101.1] — 2026-08-13

### Connector design doc sync (issue #222)

- **`VISION/03-Connectors/Technical-Design.md` rewritten to match the locked Phase 3 implementation.** The stale pre-F6 interface (authenticate with a config argument, `sync -> Vec<RawEvent>`, `extract(Vec<RawEvent>) -> Vec<ExtractedFact>`, `forget(&mut self)`) is replaced with the actual `Connector` / `ConnectorFactory` traits and `ConnectorContext` from `mimir-connectors/src/connector.rs`, the `NormalizedFact` model from `mimir-knowledge/src/normalize/types.rs`, and the real data types (`ConnectorMode`, `SyncOptions` / `SyncOutcome`, `HealthStatus`, `ConnectorAction` / `ActionResult`, `ConnectorError`). The lifecycle diagram, sync-strategy section, normalization-pipeline examples, rate-limiting pointer, and technology stack were aligned with the code (including the scoped note that no connector-side `RawEvent` / `ExtractedFact` types exist), and a source-of-truth note now points readers at the crate and `VISION/09-Roadmap/Phase-3-Plan.md`.
- Version bumped 0.101.0 → 0.101.1 (patch — backwards-compatible documentation update).

## [0.100.0] — 2026-08-12

### DRY: shared OAuth fake-browser test doubles (issue #290)

- **`mimir-connectors::test_utils` (feature `test-utils`, off by default).** The `self_callback_opener` fake-browser helper was duplicated verbatim across `mimir-connectors/src/oauth/pkce.rs` (unit tests) and `mimir/src/connector/tests.rs` (CLI add/auth e2e tests), introduced by A4 / #205. The shared module now owns the drift-prone pieces once: `parse_authorize_url` (redirect URI + CSRF state extraction), `callback_url` (code + state echo), and `self_callback_opener(code)` (drives the loopback callback). The crate's own unit tests compile the module via `cfg(test)`; downstream crates opt in with the feature (the `mimir` binary's dev-dependencies enable it). No new dependencies — the helpers only use `reqwest` and `tokio`, both already unconditional.
- **Both test suites refactored onto the shared helper.** `oauth::pkce` tests dropped their local copy and the two inline variant openers (wrong-state, favicon-probe) now build on `parse_authorize_url` / `callback_url`; `mimir/src/connector/tests.rs` imports `self_callback_opener("auth-code")` at both call sites. The e2e openers (`browser_opener` in `mimir-connectors/tests/oauth_pkce_e2e.rs`, the `$BROWSER` curl script in `mimir/tests/connector_oauth_e2e.rs`) stay separate because they must accept the mock's self-signed certificate and follow the redirect — different mechanics, not the same drift-prone code.
- **Docs:** `docs/oauth-client.md`, `docs/e2e-testing.md`, `docs/wiki/Testing-and-Benchmarks.md`, `docs/wiki/what-works-now.md` (#290 row removed), `README.md`, `AGENTS.md` (test-only feature convention), and `Mimir-Implementation-Context.md` updated.
- **Review fixes (PR #297):** `callback_url` now appends `code`/`state` as percent-encoded query pairs via a URL query builder (reserved characters such as `+`, `#`, `%`, `&` are no longer mangled, and an existing query on the redirect URI is preserved with the correct separator), the loopback test bounds every accept/read wait with a 5-second deadline so a broken opener fails fast instead of hanging, and the README documentation references are clickable links.
- Version bumped 0.99.0 → 0.100.0 (minor — backwards-compatible refactor of test infrastructure).

## [0.99.0] — 2026-08-12

### Phase 3 T2 — mock OAuth server + PKCE/rate-limit/supervisor E2E tests (issue #207)

- **In-process mock OAuth 2.0 authorization server.** `mimir-connectors::mock_oauth` (feature `test-mock-oauth`, off by default) serves the two endpoints the interactive PKCE flow needs without a real provider: an HTTPS `GET /authorize` (self-signed `rcgen` certificate generated per test run; the flow's HTTPS-only `auth_uri` gate is honoured) issues a one-time code and redirects to the loopback callback with the CSRF `state` echoed, and an HTTP `POST /token` validates the PKCE S256 `code_verifier` against the challenge captured at authorize time, enforces one-time code use, and issues an OAuth token bundle. Both endpoints record every request for assertions.
- **PKCE flow E2E against the mock server.** `mimir-connectors/tests/oauth_pkce_e2e.rs` drives `run_pkce_flow` through the full authorize → redirect → loopback callback → code-exchange round trip with a fake-browser opener, and asserts the exchanged `SecretBundle` contents plus the exact authorize/token request shapes. Mock-correctness tests cover the state echo, one-time code replay rejection, wrong-verifier rejection, and unknown grant types.
- **Daemon-level OAuth E2E.** `mimir/tests/connector_oauth_e2e.rs` drives the real `mimir connector add` CLI against the real daemon with `auth.kind=oauth` config: the CLI's `webbrowser` call is redirected to a `$BROWSER` fake-browser script (`curl -k -L`) that follows the HTTPS authorize redirect, and the exchanged tokens land in the daemon's secret store (`auth_state=authenticated`), after which the instance can be resumed and synced.
- **Rate-limit/backoff tests over real HTTP.** `mimir-connectors/tests/rate_limit_http.rs` verifies the F12 primitives against a wiremock endpoint: 429 with `Retry-After` (and 503) are retried by `retry_with_backoff` with the server hint driving the wait, and a `RateLimiter` with `daily_quota=Some(N)` stops issuing HTTP calls once the quota is spent (the exhaustion surfaces as a non-retryable `QuotaExhausted` and the wiremock `expect` proves no further request).
- **Supervisor edge-case tests.** `mimir-connectors/tests/supervisor_lifecycle_tests.rs` now covers the F8 edge cases: startup restore, graceful-shutdown cursor persistence, circuit breaker (both ordinary failures and repeated panics), and panic recovery.
- **Test harness DRY.** `TestDaemon` gains `run_cli_with_env` (extra env vars, e.g. `BROWSER` for the OAuth E2E) alongside the existing `run_cli` / `run_cli_json` helpers.
- **Docs:** `docs/e2e-testing.md` (T2 sections), `docs/oauth-client.md` (feature gating + testing), `docs/wiki/what-works-now.md` (PKCE + E2E harness rows → ✅ Works, #207 removed from the Connectors work items), and `docs/wiki/Testing-and-Benchmarks.md` updated.
- Version bumped 0.98.0 → 0.99.0 (minor — backwards-compatible new test deliverable, matching the 0.97.2 → 0.98.0 precedent for T1 / #206).

## [0.98.0] — 2026-08-12

### Phase 3 T1 — mock connector sync→normalize→insert→query E2E harness (issue #206)

- **Daemon-level fact-ingestion E2E tests.** `mimir/tests/connector_e2e.rs` now configures the `gmail/test` mock connector's `facts` knob and drives the full pipeline through the real CLI + in-process daemon: `connector add --config-json` → auth → resume → sync → `kb query` / `kb show`. The tests assert facts land with `source_type=Connector`, provenance tied to the connector instance (`connector_instance_id` + `raw_reference`), confidence from the connector reliability score (Gmail = 0.85), sync-cursor persistence, and the derived per-instance `item_count`.
- **Corroboration path exercised end-to-end.** A second connector instance corroborating the same claim merges into the existing fact row (entity resolution), adds an independent source, and boosts confidence to 0.90 (+0.05, capped at 0.95); a plain re-sync of the same instance is asserted to be a re-statement no-op (no extra source, no further boost).
- **Supervisor-level confidence assertion.** `mimir-connectors/tests/mock_ingestion_e2e.rs` now asserts the exact Gmail reliability score (0.85) instead of a loose `> 0.0` check.
- **Test harness DRY.** `TestDaemon` gains a `run_cli_json` helper (asserts success + parses JSON stdout); the existing lifecycle e2e test was refactored onto it.
- **Docs:** `docs/e2e-testing.md` (connector E2E section), `docs/mock-connector.md` (T1 status), `docs/connector-management.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md` (E2E harness row → ✅ Works), `docs/wiki/Testing-and-Benchmarks.md`, `README.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.97.2 → 0.98.0 (minor — backwards-compatible new test harness deliverable, matching the 0.94 → 0.95 precedent for the mock-connector daemon feature).

## [0.97.2] — 2026-08-12

### Review fixes — PKCE flow security hardening and OAuth config back-compat (PR #291)

- **HTTPS-only authorization endpoint.** `run_pkce_flow` now rejects any `auth_uri` whose scheme is not `https` before the browser is opened (RFC 8252 §7.5) — the authorization endpoint carries the user's credentials, so plain HTTP is never allowed there even though the token endpoint gate permits loopback HTTP.
- **Fixed callback error page.** A callback that aborts the flow (provider `error` param, missing/incorrect `state`) now responds with a fixed HTML page instead of echoing the provider-controlled `error` value into the browser (XSS on the loopback origin); the diagnostic stays in the process error only.
- **Stored OAuth configs without `auth_uri` load again.** `CalendarAuthMethod::OAuth` and `EmailAuthMethod::OAuth` persist `auth_uri` as optional (`#[serde(default)]`), so records created before the field existed (pre-0.97.0) deserialize instead of failing at startup; new configs still require `auth_uri` via the JSON schema, and the interactive PKCE flow still fails with a clear message when it is absent.
- **`oauth::pkce` is now a public module** (`mimir_connectors::oauth::pkce`), alongside the existing root re-exports.
- **OAuth progress output moved to stderr.** The printed authorize URL and the "Starting OAuth login" message go to stderr, so `mimir connector add/auth --json` stdout stays valid JSON for scripts.
- **Docs:** `docs/wiki/cli-commands.md` documents that `auth.scopes` must be supplied via `--config-json` (the `key=value` parser drops JSON arrays, #289); `docs/wiki/connectors.md` documents the required `mimir connector resume <slug>` step after `add`/re-auth; the VISION onboarding example uses the documented `mimir connector add gmail --backend <b>` syntax; `docs/oauth-client.md` security properties updated.
- **Tests:** non-https `auth_uri` rejected without opening the browser, callback error page does not echo provider input, and stored-record deserialization without `auth_uri` for both Calendar and Email.
- Version bumped 0.97.1 → 0.97.2 (patch — bug fixes, no API breakage).

## [0.97.1] — 2026-08-12

### Review fixes — PKCE loopback flow robustness and timeout UX (PR #291)

- **Per-connection read deadline on the loopback callback listener.** A connection that sends nothing (or a partial request) is now dropped after a 10-second deadline instead of stalling the whole flow until the 5-minute overall timeout — a stalled or hostile local process can no longer waste the user's authorization. The dropped connection is ignored and the flow keeps waiting for the real callback.
- **Timeout error no longer points at a dead login.** When the flow times out, the error now states that the flow aborted and the command must be re-run, instead of telling the user to complete a login whose loopback listener is already closed.
- **Tests:** stalled-connection read timeout, stalled connection followed by a real callback, and the timeout message wording.
- **Docs:** `docs/oauth-client.md` security properties and `docs/wiki/cli-commands.md` updated with the per-connection deadline and timeout behaviour.
- Version bumped 0.97.0 → 0.97.1 (patch — bug fixes, no API change).

## [0.97.0] — 2026-08-11

### Phase 3 A4 — interactive OAuth PKCE loopback flow (issue #205)

- **`mimir-connectors::oauth::pkce` — the interactive PKCE authorization-code flow.** `run_pkce_flow` binds an ephemeral loopback listener on `127.0.0.1:0`, builds the provider's authorize URL with an S256 PKCE challenge + CSRF state, receives the redirect (8 KiB read cap, state validated, favicon probes ignored), exchanges the code via the shared `OAuthHttpClient` (HTTPS/loopback token-endpoint gate + secret-hygiene error mapping), and returns the `SecretBundle::OAuth` for the caller to persist. The daemon never runs a transient HTTP server. Public surface: `PkceFlowConfig`, `run_pkce_flow`, `DEFAULT_FLOW_TIMEOUT` (gated by the `oauth` feature, which now also gates `url`).
- **`mimir connector add` / `auth` run the flow for `auth.kind=oauth` configs.** `add` acquires the credential *before* registering the instance (a canceled prompt or aborted OAuth flow exits with nothing created) and POSTs the exchanged bundle to the daemon's token-ingest route so the instance becomes `authenticated`. `auth` re-runs the flow for expired credentials, taking the OAuth client config from re-supplied `key=value` / `--config-json` args (the daemon does not expose the stored config on the wire). The authorize URL is printed before the browser is opened (`webbrowser` 1.2.4), so headless/SSH sessions can complete the login manually; a browser-open failure is non-fatal.
- **Breaking config change: `auth_uri` is now required on OAuth auth methods.** `CalendarAuthMethod::OAuth` and `EmailAuthMethod::OAuth` gained a required `auth_uri` field (the provider's authorization endpoint), reflected in the JSON schemas and config docs. Existing OAuth configs must add `auth.auth_uri` before the flow can run.
- **Tests:** 12 new `run_pkce_flow` / `parse_callback` tests (happy path, state mismatch, timeout, non-HTTPS token endpoint, invalid auth URI, favicon probe, provider-error param, percent-decoding, secret-hygiene error mapping, refresh-token retention, expiry clamping) plus CLI e2e tests for the add/auth PKCE paths against a wiremock token endpoint, config extraction, and ingest conversion.
- **Docs:** `docs/oauth-client.md` documents the flow and its security properties; connector-management, CLI, email/calendar connector, wiki, README, `Mimir-Implementation-Context.md`, and the VISION Phase 3 plan updated; `VISION/03-Connectors/User-Experience.md` onboarding example updated from paste-a-code to the loopback flow.
- Version bumped 0.96.2 → 0.97.0 (minor — new feature; breaking config change acceptable per project policy).

## [0.96.2] — 2026-08-11

### Docs — what-works-now.md rewritten as a feature-level roadmap

- **`docs/wiki/what-works-now.md` rewritten.** The changelog block in the header is removed (release history lives in this file), the Phase 3 roadmap wall-of-text is condensed to a summary, and the feature reference is expanded into an honest per-feature status guide (✅ works / 🟡 partial / ❌ not implemented) with the pending work for each feature linked to its GitHub issue. New sections cover Events & Reminders, Connectors, Background Jobs, the LLM client/worker pool, and the Librarian/Retrieval agents; the API endpoint table now matches the real route set; the Known Limitations table drops the closed #71/#45 entries and lists the current open issues.
- **Nine new GitHub issues created from the audit** (#279–#287): session compaction, chat session persistence, HTTP API authentication, LLM semantic entity dedup, email iMIP CANCEL lifecycle, memory pinning, macOS launchd, config hot-reload propagation, and a small code-quality cleanup.
- Version bumped 0.96.1 → 0.96.2 (patch — documentation only; no code or behaviour change).

## [0.96.1] — 2026-08-11

### Review fixes — oauth2/reqwest reconciliation (PR #278)

- **OAuth HTTP client built only for OAuth auth methods.** `CalendarConnector` and `EmailConnector` now construct the hardened `OAuthHttpClient` only when the config uses `CalendarAuthMethod::OAuth` / `EmailAuthMethod::OAuth` (stored as `Option`); an app-password connector no longer allocates a second reqwest connection pool and can no longer fail startup on an OAuth client build error.
- **Token-response body capped at 64 KiB.** The `OAuthHttpClient` adapter streams the token-endpoint response with an explicit bound instead of buffering it whole, so a compromised or misconfigured endpoint cannot force a large allocation on the refresh path of a long-running daemon.
- **Hostile `expires_in` clamped to 90 days.** `expires_at_from_now` no longer saturates at chrono's `MAX_UTC` (which made `needs_refresh` permanently false and reused a dead access token forever); absurd provider values now clamp to a plausible 90-day lifetime.
- **`OAuthHttpClient::from_client` narrowed to `pub(crate)`** (test-only escape hatch around the redirect hardening), and the crate-level docs no longer link to the feature-gated `oauth` module from the no-feature doc build.
- **Tests:** unknown-expiry reuse test, oversized-body rejection tests, and the network-cause assertion no longer depends on reqwest's internal `"builder error"` display string.
- **Docs:** the OAuth error contract is now stated consistently across the connector docs (provider response errors expose only parsed `error`/`error_description`; network failures include the underlying reqwest error detail; raw response bodies are never surfaced), the A4 CLI PKCE flow is marked planned/reserved in the framework table, and the Email OAuth credential model documents the optional `client_secret` (sent in the token-refresh request body when configured, never stored or logged).

## [0.96.0] — 2026-08-11

### OAuth2 / reqwest reconciliation (issue #240)

- **`oauth2` 5.0.0 joins the tree with `default-features = false` and a custom HTTP adapter.** The Phase 3 deps ledger mandates `oauth2` for Calendar/Email refresh and the A4 PKCE login, but the crate's optional `reqwest` feature pins reqwest 0.12, which would duplicate the workspace's reqwest 0.13 HTTP/TLS stack. The reconciliation: a new `OAuthHttpClient` newtype (`mimir-connectors::oauth`) implements the crate's `AsyncHttpClient` trait over the workspace reqwest 0.13 client — `HttpRequest`/`HttpResponse` are plain `http` 1.x types shared by both reqwest lines, so the adapter is the same pattern as oauth2's own `reqwest_client.rs` with no reqwest 0.12 in the tree. Gated by a new `oauth` feature (enabled by `calendar` and `gmail`; the CLI PKCE flow A4 / #205 builds on it).
- **Refresh migrated from the hand-rolled POST to the vetted grant.** The shared `oauth::refresh_token` helper now drives `oauth2`'s `exchange_refresh_token` (form-body client credentials, refresh-token rotation retained, `expires_in` → `expires_at` unchanged), and the duplicated expiry-check/refresh/persist logic in the Calendar and Email `resolve_auth` arms is DRY-extracted into `oauth::resolve_access_token` — one OAuth refresh path, ~300 lines of hand-rolled protocol code deleted.
- **Security hardening preserved and extended.** The HTTPS/loopback token-endpoint gate and the secret-hygiene error mapping (parsed `error`/`error_description` only, truncated at 256 bytes, raw response body never surfaced) carry over unchanged; the OAuth client is additionally built with `redirect::Policy::none()` so a credential POST can never be bounced to another host (the pre-#240 refresh followed redirects by default). Client credentials stay in the form body (`AuthType::RequestBody`), matching prior behaviour.
- **Dead code removed:** the Email connector's now-unused shared `reqwest::Client` field/param is gone (`from_config_with_http` → `from_config_with_deps`), and reqwest's `form` feature is dropped from `mimir-connectors` (the `oauth2` crate builds its own form-encoded body).
- **Tests:** 27 new/updated tests — wiremock refresh round-trips (request shape, token parsing, rotation, scope joining), secret-hygiene + truncation, redirect non-following (attacker host sees zero requests), HTTPS/lookalike-loopback gates, skew-window refresh, and connector-level expired-token refresh for both Calendar and Email. Full workspace suite green; clippy/fmt clean. New deps: `oauth2` 5.0.0 (+ `http` 1.x declared directly); `rand 0.8` enters the tree as a third rand line (oauth2's PKCE verifier generation; 0.9/0.10 already present) — recorded in the deps ledger.
- **Review fixes:** network-level refresh failures now surface the underlying reqwest cause — oauth2's `HttpClientError::Reqwest` display is the constant `client error` (the inner error is not part of the format string), so the adapter formats the inner error (DNS / timeout / TLS / connection) into `ConnectorError::Network` — and a hostile `expires_in` saturates at chrono's `MAX_UTC` instead of panicking the refresh path (`DateTime + TimeDelta` overflows on values beyond year 262143; `Duration::MAX` is far beyond that range).
- **Docs:** deps ledger (`VISION/09-Roadmap/Phase-3-Plan.md` §4) records the chosen path; new technical doc `docs/oauth-client.md`; `docs/calendar-connector.md`, `docs/email-connector.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` updated. `AGENTS.md` needs no change (no rule or ledger reference became stale). Version bumped 0.95.1 → 0.96.0 (minor — backwards-compatible new dependency + internal refactor). The `EmailConnector::from_config_with_http` → `from_config_with_deps` rename is a **public API change** (`EmailConnector` is re-exported from `mimir-connectors`): the injected `reqwest::Client` parameter is removed (the connector now builds its own OAuth HTTP client from config) and the final parameter is the optional LLM backend. Migrate callers to `from_config_with_deps(config, secret_store, user_identity, cursor, llm_backend)`.

## [0.95.1] — 2026-08-11

### Review fixes — `mimir connector` CLI (PR #272)

- **CLI correctness:** `connector add` now rejects `--password` together with `--token` (clap `conflicts_with`, matching `connector auth`), and a failed credential ingest after a successful `add` prints the server error plus the recovery hint `mimir connector auth <slug>` so an unauthenticated instance is never a dead end.
- **Config parsing:** `key="value"` pairs in `connector add` keep their string type — double-quoted scalars no longer coerce (`account="0755"` stays `"0755"`, `version="1.0"` stays `"1.0"`) — while unquoted values keep the existing boolean/number coercion.
- **Test hardening:** wiremock mounts in the connector unit tests now assert exact hit counts (`.expect(1)`), and the binary-level `mimir connector` tests isolate `HOME`, `XDG_CONFIG_HOME`, and `XDG_DATA_HOME` in a `tempfile` directory so they never touch the host home directory.
- **Docs:** release docs now match the CLI — `auth` added to the subcommand inventories, `--json` qualified (every subcommand except `remove`), `remove`/`forget` documented as alternative teardown operations (not a sequence), Quick Start uses one polling connector (`gmail`) with the required `resume` activation, connector status pages say library + daemon/CLI integration are implemented, and the Phase 3 roadmap summary reflects that only A4 (#205, OAuth PKCE) remains.

## [0.95.0] — 2026-08-11

### CLI — `mimir connector` subcommands (Phase 3 A3 / #204)

- **Ten connector subcommands plumb the A1/A2 daemon routes** through `mimir-client` in a new `mimir/src/connector/` module (clap definitions in `mimir/src/cli.rs`, dispatch in `main.rs`, one handler module per concern — `add`/`auth`/`query`/`lifecycle`/`sync`/`actions`). `add <type> --backend <b> [key=value...]` registers an instance in `Setup` (dotted config keys nest, `--config-json` provides a base object, scalars parse as booleans/numbers/strings); `list` and `status [slug]` render tabled/coloured overviews; `sync <slug> [--full | --since <dur>]` triggers a manual cycle (human durations `30s`/`5m`/`12h`/`7d` or bare seconds, `--full`/`--since` mutually exclusive); `pause`/`resume` control the runner; `remove` detaches provenance (facts survive) while `forget` cascade-trashes the connector's facts (recoverable 30 days), credentials, and row; `act <slug> <kind> [payload | --json-file]` dispatches write-backs (Calendar `create_event`/`update_event`/`delete_event`) and echoes the `ActionResult`. Every subcommand except `remove` supports `--json` output consistent with `kb`.
- **Non-OAuth credential ingest:** when the merged config declares `auth.kind=app_password`/`api_token`, `add` prompts for the secret via `inquire` *before* registering the instance — a canceled prompt aborts with nothing created — and ingests it through `POST /connectors/{id}/tokens` (`--password`/`--token` flags make it non-interactive; a non-interactive run without a flag registers the instance unauthenticated and warns). The new `auth <slug> [--password | --token]` subcommand re-ingests credentials on an existing instance (the recovery path for that warning and for expired credentials, without `remove` + re-`add`); the credential kind comes from the flag or an interactive selection, since the daemon does not expose the stored config on the wire. OAuth configs never prompt — the PKCE flow remains A4 (#205). Destructive subcommands confirm via `inquire` with a `--yes` skip, and `sync` surfaces the `CONNECTOR_NOT_RUNNING` 409 with an activation hint (`mimir connector resume <slug>`).
- **Slug resolution + error rendering:** slug-based subcommands resolve slugs client-side against `GET /connectors` (no by-slug route), and daemon `ApiError` JSON bodies are unwrapped so users see the human detail instead of raw JSON.
- **Shared CLI helpers (DRY):** `exit_with_error`, `make_client`, and `print_json` move to `mimir/src/cli_util.rs`, reused by both the `kb` and `connector` command groups (previously duplicated in `kb/mod.rs` and `commands.rs`).
- **`mock-connector` daemon feature:** `mimir-server` gains a `mock-connector` feature (default off) that registers the mock factory in the daemon registry (previously `cfg(test)`-only), enabling a real CLI e2e cycle — `mimir/tests/connector_e2e.rs` runs add → status → auth → resume → sync → pause → resume → remove on one instance, then add → forget on a second (remove deletes the row, so forget always targets a fresh instance) against an in-process daemon with the mock backend (the `auth` step asserts the credential ingest flips `auth_state` to `authenticated`); `mimir/tests/common/mod.rs` extracts the shared `TestDaemon` fixture from `e2e.rs` (DRY), and `mimir/tests/connector_cli_tests.rs` covers the full clap → daemon-guard → HTTP path against wiremock. 30 new tests total (25 unit/wiremock, 4 binary-level, 1 e2e).
- **Docs:** `docs/cli.md` (new `mimir connector` reference + corrected "direct library linkage" overview), `docs/connector-management.md` (A3 section), `docs/wiki/cli-commands.md` (connector section), `docs/wiki/connectors.md` (CLI usage), `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` updated; stale `A4 / #206` references corrected to `#205` across connector docs. Version bumped 0.94.0 → 0.95.0 (minor — backwards-compatible new feature).

## [0.94.0] — 2026-08-10

### Module-split refactor

- **Oversized files broken into single-concern modules:** every file that mixed multiple responsibilities is now a directory of small modules with the public API re-exported unchanged from the root module. In `mimir-connectors`: `supervisor` (`config`/`error`/`trigger`/`runner`/`control`/`cycle`), `calendar` (`construct`/`credentials`/`sync`/`trait_impl`/`payload` + `caldav/`), `email` (`config`/`factory`/`imap`/`connector/`/`jsonld/`/`llm/`), `rate_limit/`, `geocoder/`, `ical/`, `mock/`, `photos/`, and `secrets` (`error`/`bundle`/`store`/`file`/`memory`). In `mimir-core`: `config/`, `context/`, `job_queue/`, `llm/client/`, and `llm/pool/`. In `mimir-knowledge`: `extract/`, `normalize/`, `forget/`, `queries/entity/`, `queries/fact/`, `queries/memory/`, `queries/preference/`, and `optimization/`. In `mimir-server`: `routes/kb/`, `state/`, plus `app.rs`/`server.rs`/`shutdown.rs` extracted from `lib.rs`. In `mimir-client`: `kb/`. In `mimir-api-types`: `chat.rs`/`connectors.rs`/`kb.rs`/`kb_maintenance.rs`.
- **Integration suites split per feature:** `fact_management_test.rs` (2627 lines) → eight fact-focused suites; `integration_tests.rs`, `extraction_test.rs`, `calendar_connector.rs`, `supervisor_lifecycle.rs`, `kb_tests.rs`, and `chat_tests.rs` each split into per-concern files with shared fixtures in `tests/common/`.
- **Documentation refresh:** `docs/refactoring-module-split.md` and `docs/wiki/module-split.md` document the new module map and rationale; subsystem docs (`workspace`, `chat-server`, `config-system`, `llm-client`, `knowledge-graph-schema`, `connectors-framework`, and others) now reference the new module locations.
- **Quality pass:** zero-warning `cargo clippy --workspace --all-targets` (fixed orphaned doc-comment blocks, `module_inception` via `supervisor/runner.rs`, `too_many_arguments` allowances, and a `doc_lazy_continuation`); `cargo fmt --all` clean; full workspace suite 1476 passed / 0 failed. Version bumped 0.93.0 → 0.94.0 (minor — refactor, no public API or behaviour changes).

## [0.93.0] — 2026-08-10

### Connectors — PR #268 review feedback

- **Forget cascade invokes `Connector::forget()` (contract fix):** the daemon's forget route now calls a new `ConnectorSupervisor::forget(id)` which stops the runner and invokes the connector's local `forget()` cleanup on the live instance (or a freshly re-instantiated one when no runner is alive), honouring the `Connector` trait contract that connector-local cleanup is the supervisor's job. The cascade is serialised per connector via a new `ConnectorSupervisor::lifecycle_lock(id)` (also acquired by `start`/`resume`), marks the instance `Paused` first so an aborted cascade leaves a state a retry can reason about, and deletes the secret *before* the irreversible fact trash so a credential-deletion failure aborts with nothing destroyed.
- **Credential-ingest and forget routes are loopback-only:** `POST /connectors/{id}/tokens` and `/forget` now carry the `require_loopback` layer like the other sensitive/destructive endpoints; a non-loopback caller gets `403` before any mutation (new route test).
- **OAuth failure details masked in `401` responses:** `ConnectorError::Authentication` now returns the fixed `"authentication failed"` body (full detail stays in the server log) so a provider-echoed token-endpoint response can never leak credentials back to the caller (new unit test).
- **`IngestTokenRequest` gets a redacting `Debug` impl:** the derived `Debug` printed `access_token` / `refresh_token` / `token` / `password` verbatim; the manual impl prints `<redacted>` while keeping variant tags and optional-field presence visible (new unit test).
- **`spawn_into` now enforces its documented invariant:** it stops any existing runner for the row before inserting the new handle, so a re-spawn can never detach a live task; `start`'s redundant pre-stop is removed (the lifecycle lock now serialises against the forget cascade).
- **`ActError` gains `From<SupervisorError>`:** the manual `map_err` at the dispatch site is replaced with `?`, so a new `SupervisorError` variant is caught by the compiler instead of silently falling through.
- **Shared batch-trash helper in `mimir-knowledge`:** the third verbatim copy of the chunked trash loop is extracted into `forget::trash_ids_in_batches` (with a `TRASH_BATCH_SIZE` const), used by `forget_facts`, `forget_all`, and `forget_facts_for_connector`.
- **Test hardening:** fixed 50 ms sleeps in supervisor/server tests are replaced with polling helpers (`wait_for_status` / `wait_for_running` / `wait_for_runner_exit`) so loaded CI runners cannot flake; new tests cover multi-source fact trash, `forget` on live / cold / unknown instances, secret deletion in the forget route, and non-loopback rejection.
- **Docs:** `docs/connector-management.md` and `docs/wiki/connectors.md` classify `act` as an action method (not lifecycle), document `start` as an internal supervisor method, refresh the backend status (Calendar/Email extraction + write-back shipped), describe the hardened forget cascade, and remove the remaining A1-era status text; `docs/wiki/what-works-now.md` documents natural runner-exit recovery. Version bumped 0.92.0 → 0.93.0 (patch — review fixes only).
- **Round 2:** `ConnectorSupervisor::forget` now captures the live connector *before* stopping the runner (previously `stop()` removed the handle first, so the live instance was never used and the connector was always re-instantiated — the in-memory state the cleanup must tear down, e.g. the Photos watcher, was dropped instead); the live-path test now asserts the factory is called exactly once. `docs/wiki/connectors.md` fixes the OAuth PKCE issue reference (`#206` → `#205`), and `docs/wiki/what-works-now.md` metadata aligns with 0.93.0 / 2026-08-10.

## [0.92.0] — 2026-08-06

### Connectors — action routes + OAuth token ingest + forget cascade (A2 / #203)

- **Action routes:** six new Axum routes round-trip via `mimir-client` — `POST /connectors/{id}/sync` (manual sync trigger, F9; maps the supervisor's `TriggerOutcome` to a `status`-tagged `SyncConnectorResponse`), `POST /connectors/{id}/pause` and `/resume` (lifecycle control), `POST /connectors/{id}/tokens` (ingest a `SecretBundle` keyed by slug via the shared `SecretStore` and flip `auth_state` to `authenticated`), `POST /connectors/{id}/actions` (dispatch `{ kind, payload }` to the connector's `act()` write-back, e.g. the Calendar connector's `create_event`/`update_event`/`delete_event`), and `POST /connectors/{id}/forget` (cascade-forget: stop the runner → trash every fact sourced from the connector via the existing trash machinery → delete the slug-keyed secret → delete the row). `forget` is recoverable from trash for 30 days, unlike `DELETE /connectors/{id}` (A1) which detaches provenance so facts survive with degraded provenance.
- **Supervisor additions (`mimir-connectors`):** `ConnectorSupervisor::start(id)` re-spawns a single connector (load row → instantiate → stop any existing runner → flip `Active` → spawn), shared via a new private `spawn_into` helper used by both `restore()` and `start()`. `pause(id)`, `resume(id)`, and `act(id, action)` land; each `ConnectorHandle` now retains the live `Arc<dyn Connector>` (cloned from the one moved into the runner) so write-back actions dispatch to the authenticated instance. `act` re-instantiates from the row when no live runner exists — including a connector whose runner exited naturally (auth-expiry, circuit-breaker, panic), whose stale handle is dropped first — so a write-back never runs against expired in-memory credentials after fresh ones are stored via `/tokens`. New `SupervisorError::UnknownConnectorType` variant and a new `ActError` enum.
- **Knowledge-graph facade (`mimir-knowledge`):** `KnowledgeGraph::forget_connector_facts(id, changed_by)` — the `forget` cascade. Soft-deletes (trashes) every fact whose `sources` row carries `connector_instance_id = id` via the shared `forget_fact_tx` trash machinery; `sources` rows are cascade-deleted with their facts (`ON DELETE CASCADE`); no `--yes`/`--confirm-sensitive` gate applies (an explicit admin action). Backed by a new `forget::forget_facts_for_connector` function.
- **Wire types (`mimir-api-types`):** `SyncConnectorRequest`/`SyncConnectorResponse`, `IngestTokenRequest` (a `kind`-tagged mirror of `SecretBundle`, keeping `mimir-api-types` decoupled from `mimir-connectors`), `ConnectorActionRequest`/`ActionResultResponse`, `ForgetConnectorResponse`, with round-trip tests.
- **Server error mapping (`mimir-server`):** a `ConnectorError` → HTTP mapping (`UnsupportedAction`/`Config`/`Parse`→400, `NotAuthenticated`/`Authentication`→401, `Network`→502, `BackendNotFound`→404, `BackendAlreadyRegistered`→409, `Io`/`Other`→500 with detail masked) plus `SupervisorError`, `ActError`, `TriggerError`, and `SecretError` mappings, with unit tests.
- **Mock connector:** `MockConnector` gains an `act_kind` config so the action-dispatch path is testable; unsupported kinds return `UnsupportedAction`.
- **Client (`mimir-client`):** `connector_sync`, `connector_pause`, `connector_resume`, `connector_tokens`, `connector_actions`, `connector_forget` methods.
- **Docs:** updated `docs/connector-management.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` to reflect A2 landing. Version bumped 0.91.0 → 0.92.0 (minor — backwards-compatible new feature routes and facade methods).



### Connectors — PR #263 review feedback (round 2)

- **Delete stored credentials with the connector instance (security):** `DELETE /connectors/{id}` now deletes the connector's slug-keyed `SecretStore` entry as well as the row. Previously the secret lingered after the row was removed, so a later connector created with the same slug could load the deleted instance's credentials. The secret is deleted *before* the row using a new `ConnectorSupervisor::secret_store()` accessor, and `SecretStore::delete` is idempotent (a missing entry is `Ok`), so an instance that never stored credentials cleans up as a no-op. A secret-deletion failure aborts the removal (`500`) and leaves the instance intact, so the database and secret store are never left in an ambiguous state and the request never reports success while a credential lingers. New route test: deleting an authenticated connector removes its credential, verified by loading it again and by re-creating a same-slug connector that cannot load the old secret.
- **Return the connector-not-found detail (functional correctness):** `KnowledgeError::ConnectorNotFound` was mapped to `404` but the message branch omitted it, so deleting an unknown connector returned the generic `"internal knowledge graph error"` instead of the not-found detail. It now preserves its detail (`"Connector {id} not found"`) like the other not-found variants. New unit test for the mapping and a route-level `DELETE /connectors/{id}` 404 test.
- **Docs:** updated stale `v0.89.0` release references to `v0.90.0` in `docs/wiki/connectors.md`, the `docs/wiki/what-works-now.md` version banner, and `README.md`; documented the secret-deletion flow in `docs/connector-management.md` and `docs/wiki/connectors.md`.

## [0.90.0] — 2026-08-06

### Connectors — PR #263 review fixes

- **Atomic connector creation (data integrity):** `POST /connectors` no longer pre-reads `get_connector_by_slug` then calls `upsert_connector` (a read-then-write window that let two concurrent same-slug writes both pass the read and let the later one update the first row instead of returning `409`). A new `KnowledgeGraph::create_connector` / `queries::connector::create_connector` does a plain `INSERT` with no `ON CONFLICT` clause and relies on the `connectors.slug UNIQUE` index to reject a duplicate at the database level, mapping the unique violation to a new `KnowledgeError::ConnectorSlugConflict` (mapped to `409 Conflict` by the server error layer, preserving the existing `connector slug '...' already exists` message). `upsert_connector` is retained for the A2 / #203 reconfigure-an-existing-instance flow. New tests: a KnowledgeGraph-level concurrent same-slug create asserting exactly one winner, and a route-level concurrent `POST /connectors` asserting exactly one `201` and one `409`.
- **`ConnectorSupervisor::stop` returns `false` for an already-finished runner (functional correctness):** previously `stop` removed a stale handle whose task had completed naturally (e.g. an unauthenticated connector whose runner exited at the auth handshake) and returned `true`, contradicting its own doc that `false` means "no live runner exists (already finished, never spawned, or previously stopped)". It now distinguishes a live runner (abort + await + `true`) from a finished one (clean up the stale handle + `false`); the `None` path is unchanged. The `MockConnector` gains an `auth_fail` config flag so the supervisor lifecycle tests can drive the already-finished-handle path.

### Docs

- `docs/connector-management.md`: corrected the `GET /connectors` list-route description (it uses one `count_sources_by_connector` `GROUP BY` query, not a per-row `count_sources_for_connector` query), documented `count_sources_by_connector` in the knowledge-graph additions, and updated the `POST` route notes, supervisor `stop` description, and tests section to reflect the atomic create and finished-runner behaviour.
- `docs/wiki/what-works-now.md`: fixed a broken code span in the Phase 3 entry (`GET/DELETE /connectors/{id}`).
- `docs/wiki/connectors.md`: noted that `POST /connectors` slug uniqueness is enforced atomically by the `slug UNIQUE` index.

## [0.89.0] — 2026-08-05

### Connectors — daemon wiring + connector CRUD/status routes (A1 / #202)

- **Daemon owns the connector framework at startup:** `AppState::from_config_with_llm` now constructs a `ConnectorRegistry`, registers the built-in Photos (`local`), Calendar (`caldav`), and Email (`imap`) factories behind forwarded cargo features on `mimir-server`, builds a `ConnectorSupervisor` subscribed to the daemon-wide shutdown watch, and chains `with_secret_store(FileSecretStore)` (best-effort), `with_geocoder` (the same `Arc<dyn Geocoder>` the knowledge graph holds), `with_user_identity(cfg.identity.name)` (C4 / #198), and `with_llm_backend(llm_client)` (C7 / #201 — enables the Email prose-extraction system-queue path). `Active` connector runners are restored at startup and drained on graceful shutdown via `AppState::shutdown`. `mimir-server` forwards `photos`/`calendar`/`gmail` features to `mimir-connectors` so each factory registration is gated by the same flag that compiles the backend module.
- **Connector CRUD/status routes:** four Axum routes round-trip via `mimir-client` — `GET /connectors` (list with derived item counts), `POST /connectors` (add-only; validates the `(connector_type, backend)` pair against the registry, rejects an existing slug with `409`, rejects an unregistered backend or unknown type with `400`, creates the instance in `Setup`), `GET /connectors/{id}` (show with item count; `404` when missing), and `DELETE /connectors/{id}` (stops the runner and deletes the row; `204`). Activation, pause/resume, OAuth, the `forget` cascade, and the `mimir connector` CLI land in A2–A4 (#203–#205).
- **`ConnectorSupervisor::stop(id)`:** per-instance counterpart of `shutdown()` — aborts a single runner and removes it from the handle map (a no-op returning `false` when no live runner exists). `DELETE` uses it so a mid-cycle sync cannot write back to a vanishing row.

### Knowledge graph — connector provenance support

- **`KnowledgeGraph::count_sources_for_connector(id)`:** the derived "items ingested" metric surfaced by the connector status routes (`SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?`). The `connectors` table stores no count column; the value is computed on demand.
- **`KnowledgeGraph::delete_connector(id)`:** nulls every `sources.connector_instance_id` referencing the row (the FK has no `ON DELETE` clause, so a raw `DELETE` would violate it) then deletes the row, in one transaction. Ingested facts survive with degraded provenance; the full `forget` cascade is deferred to A2 / #203. Returns `ConnectorNotFound` when no row matches. `ConnectorNotFound` is now mapped to `404` by the server error layer.

### API surface

- New wire types in `mimir-api-types`: `AddConnectorRequest`, `ConnectorResponse` (carries `item_count`, lowercase-string `status`/`auth_state`, RFC-3339 timestamps), `ConnectorListResponse`. `mimir-api-types` stays decoupled from `mimir-knowledge`, so the connector kind/status are strings mapped to enums in the route layer.
- New `mimir-client` methods: `connectors`, `connector`, `connector_add`, `connector_remove`.

### Docs

- New `docs/connector-management.md` (technical). Updated `docs/wiki/connectors.md` (Managing connectors section), `docs/wiki/server.md` (endpoints), `docs/wiki/what-works-now.md` (routes, version, Phase 3 status), `README.md`, and `Mimir-Implementation-Context.md`.

## [0.88.0] — 2026-08-05

### Connectors — Email LLM extraction (C7 / #201)

- **Email connector LLM extraction layer (cascade layer 3):** the IMAP Email connector's `extract()` cascade gains a third, last-resort layer for unstructured prose that deterministic layers cannot read — a dentist's "see you Tuesday 3pm" with no `.ics`, a flight confirmation in prose, a bank statement, a job offer. Layer 3 runs only for messages that layers 1 (iMIP invites / #200) and 2 (`schema.org` JSON-LD / #249) produced no facts for, so a deterministic layer that already read the email is never re-processed by the LLM (avoids duplicate extraction and bounds LLM cost). When no `LlmBackend` is injected (daemon wiring is #202), layer 3 is skipped, leaving deterministic extraction unchanged.
- **"Logic in Rust, not prompts" enforced for connector LLM data:** the LLM must call a strict `extract_email_facts` tool (a closed JSON Schema) and Rust validates the tool-call name (rejecting an unexpected function or a multi-call completion) and every field against the typed enums before building `NormalizedFact`s — entity types, temporal bounds, an `event_type` hint (mapped against the `EventType` enum, dropped if unrecognised, never trusted raw), recurrence, and the location overlay — reusing the shared `mimir-knowledge::extract` parsing helpers (DRY with the conversational `remember` path). User-scoped subjects are canonicalised to the injected `user_identity` so they resolve to the canonical user entity (matching the C4/#198 and #249 layers). All facts carry `source_type = Connector`, `extraction_method = LlmExtraction`, and the email's `UIDVALIDITY`-qualified IMAP UID as `raw_reference`. The body sent to the LLM is the first `text/plain` part (or HTML stripped of markup), capped at 8 KiB.
- **Deterministic spam pre-filter:** obvious bulk-marketing mail is skipped before any LLM call by a Rust `is_likely_spam` gate: a message is skipped when it carries a `List-Unsubscribe` header (the universal bulk-mail signal) or is sent from a pure marketing platform domain (mailchimp/hubspot/mailerlite/etc.). General-purpose ESPs that also deliver transactional mail (sendgrid/mailgun/postmark/amazonses) are skipped only with the unsubscribe signal, so statements, bookings, and offers routed through them still reach the LLM. The LLM only does fact extraction and returns an empty `facts` array for no-fact prose.
- **Retryable LLM-failure handling:** a queue-full, network, provider, or parse failure during prose extraction is propagated as a `ConnectorError` (via a new `From<LlmError> for ConnectorError`) and the affected raw email is re-staged in the connector buffer for the next extraction cycle, rather than being silently converted into an empty (successful) extraction that would lose the message forever (the buffer was drained and the IMAP cursor advanced). Deterministic facts already collected this cycle are kept, so a transient LLM error never blocks them; a durable terminal-failure policy is follow-up work.

### LLM backend — system-queue tool-calling (#201 acceptance)

- **`LlmBackend::system_chat_message` / `system_chat` trait methods:** every connector LLM call routes through the shared `LlmWorkerPool`'s **system queue** (priority below user chat), so a one-call-at-a-time provider is never starved by an extraction burst and a queued user chat preempts a waiting connector call. The default implementation delegates to the user-queue `chat_message` for backends without a pool (mocks, model-override clones, direct test clients). `LlmClient` overrides it to enqueue on the system queue. The system-queue enqueue methods (`enqueue_system_chat_message` / `enqueue_system_chat` / `enqueue_system_chat_stream`) now accept a `tools` argument — previously the system queue hardcoded `tools: None`, so connector tool-calling could not use the system queue. `MockLlmClient` records `system_chat_message` calls separately from user-queue `chat_message` calls so tests assert the routing directly.

### Knowledge graph — per-fact `ExtractionMethod` (#234)

- **`NormalizedFact::extraction_method` per-fact override:** `ExtractionMethod` moves onto `NormalizedFact` as an `Option<ExtractionMethod>` that defaults to (inherits) the batch `Provenance`'s method when unset. This lets a single mixed-method `extract()` batch record the right method per fact in `sources.extraction_method_id`, fixing the `ConnectorSupervisor::run_cycle` hardcoded `ExtractionMethod::StructuredParse` (#234 / #254) that would have mislabelled C7's LLM-extracted facts as structured-parsed. The email cascade tags its facts per layer: layers 1–2 set `StructuredParse`, layer 3 sets `LlmExtraction`. The conversational `remember` path tags its facts `LlmExtraction`. Existing producers are unchanged (inherit the provenance default).

### Connectors — shared LLM backend injection

- **`ConnectorContext::llm_backend` + `ConnectorSupervisor::with_llm_backend`:** the shared `Arc<dyn LlmBackend>` is injected into connectors at construction (mirroring the existing `with_geocoder` / `with_secret_store` / `with_user_identity` builders), and the Email connector clones it at construction. Daemon wiring that calls `with_llm_backend` lands with #202. No new dependencies: the LLM layer reuses `mimir-core::llm` and `mimir-knowledge::extract`; tests use `mimir-core`'s `mock-llm` feature (now a `mimir-connectors` dev-dependency).

### Docs

- `docs/email-connector.md`, `docs/wiki/email-connector.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` updated for the C7 layer, the system-queue tool-calling, and the per-fact `ExtractionMethod`.

## [0.87.0] — 2026-08-04

### Bug fixes

- **Configurable knowledge-graph + job-queue database paths (#233):** `knowledge.db_path` and `scheduler.db_path` are now configurable (and overridable via `MIMIR_KNOWLEDGE_DB_PATH` / `MIMIR_JOBS_DB_PATH`), mirroring the existing `context.db_path`. Previously `AppState::from_config_with_llm` hardcoded the knowledge-graph and job-queue databases to the shared Mimir data directory, ignoring any config override, so the in-process e2e daemon opened the developer's real `~/.local/share/mimir/knowledge.db`. Knowledge-graph backups are now written alongside the (possibly overridden) knowledge DB instead of always escaping to the shared data dir.
- **E2E tests isolate every database to the tempdir (#251, #256, #237):** `mimir/tests/e2e.rs` now points the in-process daemon at tempdir paths for `context.db`, `knowledge.db`, and `jobs.db`. The suite no longer touches or migrates the developer's real knowledge/job databases, eliminating the `migration 45 was previously applied but has been modified` checksum-mismatch failure that made `e2e_ask_no_stream_round_trip` always fail on machines that had run `mimir start` before.
- **Fix flaky `entity_locations` "database is locked" (#236):** the background location-overlay worker and the ingestion caller (`normalize_and_insert`) now serialise their SQLite write transactions through a shared `KnowledgeGraph::write_lock`. In WAL mode a deferred read-then-write transaction that has another connection commit between its read and its write is rejected with an immediate, un-retriable `SQLITE_BUSY` (the snapshot is stale, so `busy_timeout` cannot help), which silently dropped a location overlay or failed an `insert_fact` under interleaving. The lock is held per-fact across the ingestion write and across the worker's `upsert_location` + `ensure_place_coordinates` writes, but not across the geocode network call, so off-thread geocoding throughput is preserved and reads stay fully concurrent.

### Docs

- `docs/config-system.md`, `docs/wiki/configuration.md`, `docs/entity-locations.md`, `docs/wiki/entity-locations.md`, and `docs/wiki/e2e-tests.md` updated for the new database-path overrides and the write-serialisation fix.

## [0.86.2] — 2026-08-04

### Connectors — Email JSON-LD extraction review follow-up (PR #257)

- **Functional correctness (textual lodging addresses):** `LodgingReservation` location extraction now accepts a scalar textual `address` value (schema.org permits `address` as plain `Text` in addition to a `PostalAddress` object). Previously `a.as_object()?` dropped scalar addresses so the lodging-name fallback discarded a valid `located_in` fact. Scalar addresses are now coerced via `scalar_string` before the `streetAddress` lookup, while structured `PostalAddress` handling and the self-referential `located_in` skip are preserved. A regression test covering a textual address that produces the expected `located_in` fact is added.

## [0.86.1] — 2026-08-04

### Connectors — Email JSON-LD extraction review follow-up (PR #257, #249)

- **Data integrity (reservation start-time gating):** the primary `Appointment`-typed JSON-LD reservation facts (`FlightReservation` `has_flight`, `LodgingReservation` `has_booking`, `EventReservation` `has_event`) now require a parseable start time (`departureTime` / `checkinDate` / `startDate`) before emission, matching the iMIP layer's `DTSTART` requirement so the events subsystem cannot create an appointment overlay without a `valid_from`. Secondary facts (airports, airlines, venues) are still always emitted.
- **Data integrity (self-referential `located_in`):** `LodgingReservation` no longer emits a `located_in` fact when the resolved location equals the booking name (e.g. `Grand Hotel located_in Grand Hotel`); a distinct address is required.
- **Functional correctness (numeric identifiers):** `string_or_name_field` and the `iataCode` / `flightNumber` lookups now accept JSON numbers (producers commonly emit `orderNumber` / `trackingNumber` / `ticketNumber` as numbers) and trim array-wrapped string values, via a new shared `scalar_string` helper. Numeric `orderNumber`, `trackingNumber`, `ticketNumber`, `flightNumber`, and `iataCode` no longer drop their fact cluster.
- **Functional correctness (datetime parsing):** `parse_datetime` now accepts naive datetimes with fractional seconds (`...T10:00:00.500`) and minute-only precision (`...T10:00`) in addition to RFC 3339, second-precision naive, and date-only inputs.
- **HTML scanner (spec compliance):** `<script type="...">` attribute values are now trimmed before the `application/ld+json` comparison, matching the HTML5 rule that browsers strip ASCII whitespace from the `type` attribute.
- **Encapsulation:** `mimir-connectors/src/email/mod.rs` narrows `pub mod jsonld;` to `pub(crate) mod jsonld;` (the module has no out-of-crate references).
- **Docs:** `docs/email-connector.md` and `README.md` updated (cascade layer count corrected; C7 / #201 marked as still open; emission rules and numeric/datetime handling documented). A known limitation — iMIP + JSON-LD both firing on one email can produce duplicate `Event` entities — is now documented inline in `extract()`.
- **Tests:** new unit tests cover the start-time gating (flight/lodging/event), the self-referential `located_in` skip, numeric identifiers (order/tracking/ticket/flight/iata), trimmed array values, padded `type` attribute, and the fractional/minute-only datetime shapes.

## [0.86.0] — 2026-08-04

### Connectors — Email schema.org JSON-LD deterministic extraction (#249)

- **New extraction layer:** `EmailConnector::extract()` gains a second deterministic (structured-parse) layer that scans `text/html` MIME parts for `<script type="application/ld+json">` blocks and extracts typed fact clusters for recognised `schema.org` types. This sits as layer 2 of the extraction cascade, between the iMIP calendar-invite layer (layer 1, #200) and the C7 LLM layer (#201, still open). No LLM — pure Rust parsing per the project rule "logic in Rust, not prompts".
- **Recognised types:** `FlightReservation` (`user has_flight <flight>` typed `EventType::Appointment`, plus `departs_from` / `arrives_at` / `operated_by`), `LodgingReservation` (`has_booking`, `Appointment`, `located_in`), `EventReservation` (`has_event`, `Appointment`, `located_in`), `Order` (`has_order`, plus `purchased_from`), `ParcelDelivery` (`has_delivery` typed `Reminder`, plus `shipped_by` / `delivered_to`), `Ticket` (`has_ticket`, plus `issued_by`), and `ReservationPackage` (flattens `subReservation` for multi-leg flights). Unrecognised `@type` values are logged at `debug` level and skipped — never guessed.
- **Provenance and user identity:** facts carry `source_type = Connector`, `extraction_method = StructuredParse`, and the email's `UIDVALIDITY`-qualified IMAP UID as `raw_reference` (matching the iMIP layer). The primary user-scoped fact is only emitted when a canonical user identity is configured (`ConnectorContext::user_identity`); secondary facts (airports, airlines, venues, carriers, merchants) are always emitted. Duplicate/re-sent transactional emails dedupe via the existing `normalize_and_insert` corroboration/supersession.
- **New module:** `mimir-connectors/src/email/jsonld.rs` (gated by the `gmail` feature). The HTML `<script>` scanner is hand-rolled (spec-correct: HTML5 script content terminates at the first `</script>` end tag) — no new HTML parser dependency. The shared `vevent_fact` helper in `ical.rs` is made `pub(crate)` for DRY reuse (JSON-LD facts delegate to it with `RecurrenceType::None`).
- **No new dependencies:** the module reuses the existing `serde_json`, `mail-parser`, and `chrono` crates.
- **Tests:** unit tests for each recognised type, the HTML `<script>` scanner (standard, single-quoted, multi-attribute, case-insensitive, JavaScript-skipping, `data-type`-not-`type`), JSON-LD structural normalization (`@graph`, arrays, context wrappers), a cascade integration test (iMIP + JSON-LD from one email), and a KB integration test (flight fact entity resolution + `Appointment` events-subsystem overlay + connector provenance).
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, `docs/wiki/what-works-now.md`, and `README.md` updated.

## [0.85.2] — 2026-08-04

### Connectors — Email iMIP extraction review follow-up 2 (PR #253, #200)

- **Data integrity (iMIP `METHOD` conflict):** `EmailConnector::extract_invites` (`mimir-connectors/src/email/mod.rs`) now normalises the MIME `Content-Type` `method` parameter and the iCalendar body `METHOD` property independently, and rejects (skips) a `text/calendar` part when both are present and disagree instead of silently preferring the MIME value. This prevents appointment facts being created from a part whose body says `METHOD:CANCEL`/`METHOD:PUBLISH` while the MIME header claims `REQUEST`/`REPLY` (RFC 6047 §2.4 requires the two to match when both are supplied). Matching values and single-source values keep their existing behaviour; `PUBLISH` and `CANCEL` remain skipped. New regression tests cover conflicting supported/unsupported pairs, the body-only fallback, and the no-source case.
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, and `README.md` updated to match the extraction contract — eligible parts are any `text/calendar` MIME part (not just attachments), `METHOD` is resolved from the MIME parameter or the calendar body, the `has_event` primary fact depends on a configured `ConnectorContext::user_identity`, supported `REQUEST`/`REPLY` invitations produce facts regardless of sender, and provenance is the `UIDVALIDITY`-qualified IMAP UID.

## [0.85.1] — 2026-08-04

### Connectors — Email iMIP extraction review follow-up (PR #253, #200)

- **Functional correctness (iMIP MIME walk):** `EmailConnector::extract_invites` (`mimir-connectors/src/email/mod.rs`) now walks every MIME part (`message.parts`) instead of `attachments()`, so a `text/calendar` part nested in `multipart/alternative` with no `Content-Disposition: attachment` header (classified as a body part by `mail-parser`) is no longer missed.
- **Functional correctness (METHOD fallback):** the iMIP `METHOD` is now resolved from the MIME `Content-Type` `method` parameter when present, falling back to the iCalendar body `METHOD` property (RFC 6047 §2.4 makes the parameter optional). Only `REQUEST`/`REPLY` are extracted; `PUBLISH`/`CANCEL` are still skipped.
- **Data integrity (globally-unique provenance):** the email provenance `raw_reference` is now `{uid_validity}:{uid}` (matching the persisted cursor format), not a bare IMAP UID that is unique only within one mailbox + `UIDVALIDITY` epoch. `imap::RawEmail` gains a `uid_validity` field populated by `fetch_since`.
- **Performance (buffer lock):** `EmailConnector::extract()` now drains the staged buffer and releases the mutex guard before the CPU-bound MIME parse loop, so a concurrent `sync()` cycle is not blocked from staging new mail during parsing.
- **Data integrity (iCalendar name matching):** `parse_ical_to_vevents` (`mimir-connectors/src/ical.rs`) now looks up VEVENT properties case-insensitively (`icalendar` 0.17.x `find_prop` matches case-sensitively, but RFC 5545 names are case-insensitive), and the `VEVENT` component match is case-insensitive. `participant_display` strips the `mailto:` scheme case-insensitively (`MAILTO:` occurs in the wild, per RFC 3986).
- **Tests:** the iMIP invite fixture now uses CRLF line endings (RFC 5322/MIME + IMAP `BODY.PEEK[]` wire format); the tautological `assert!(dr_smith > 0)` is replaced by a real check that the Dr Smith `attending` fact resolved to the right Person entity and points at the appointment Event; the fake-IMAP → `extract()` provenance assertion updated for the `UIDVALIDITY`-qualified `raw_reference`.
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, and `README.md` corrected — `mail-parser` is Gmail-specific while `icalendar`/`chrono-tz` are shared with `calendar`; the wiki states the `has_event` primary fact depends on a configured `user_identity`, that only supported iMIP `REQUEST`/`REPLY` parts produce facts, and that provenance is the IMAP UID; the README splits future-work references (`schema.org` JSON-LD = #249, LLM free-text extraction = C7 / #201).

## [0.85.0] — 2026-07-31

### Connectors — Email structured extraction (C6 / #200)

- **Structured extraction cascade:** `EmailConnector::extract()` (`mimir-connectors/src/email/mod.rs`) now drains staged RFC 822 messages and runs a deterministic extraction cascade over each. Today the cascade has one layer — iMIP calendar invites: a MIME attachment with `Content-Type: text/calendar; method=REQUEST|REPLY` is parsed with `mail-parser` and the embedded VEVENT is turned into the same appointment fact cluster the Calendar connector emits (`user has_event <event>` typed `EventType::Appointment`, recurrence from `RRULE` `FREQ`, temporal bounds from `DTSTART`/`DTEND`, plus `<event> located_in <place>` and `<attendee> attending <event>`). `method=PUBLISH` (often marketing webinars) and `CANCEL` (deletion lifecycle, tracked in #247) are skipped. A plain prose email with no `text/calendar` part produces no facts.
- **Email is provenance, not the fact:** no per-email communication facts (`received_email_from` / `sent_email_to`) are emitted and no `Person` entities are auto-created from `From`/`To` headers, so marketing/spam produces no junk. The email's IMAP UID rides on every fact as the `raw_reference`; facts carry `source_type = Connector`, `connector_type = Gmail`, `extraction_method = StructuredParse`.
- **DRY — shared `ical` module:** the VEVENT parsing + fact cluster is extracted into a new `mimir-connectors/src/ical.rs` (`parse_ical_to_vevents`, `vevent_to_facts`, `RawVEvent`), gated `any(feature = "calendar", feature = "gmail")`. The Calendar connector (`caldav.rs` `RawCalDavEvent` now wraps a `RawVEvent`; `calendar/mod.rs` `event_to_facts` delegates) reuses it, eliminating the duplicated VEVENT parsing + `rrule_to_recurrence` + `calendar_fact` helpers.
- **User identity:** the Email connector now authors user-scoped facts against the injected `ConnectorContext::user_identity` (the `config.toml` `[identity] name`), matching the Calendar connector; `from_config_with_http` gains a `user_identity` parameter and the factory passes `ctx.user_identity`. Without an identity the primary `has_event` fact is skipped; location/attendee facts still emit.
- **Dependencies:** `mail-parser 0.11.5` added to `mimir-connectors` under the `gmail` feature; `icalendar` 0.17 and `chrono-tz` 0.10 are now shared with the `calendar` feature (DRY) so the `gmail` feature can parse iMIP VEVENTs.
- **Tests:** unit tests for iMIP `REQUEST`/`REPLY`/`PUBLISH`/`CANCEL` gating, the no-identity path, and plain/marketing email → no facts; a knowledge-graph integration test staging an invite through `extract()` → `normalize_and_insert` asserting F5 entity resolution (user / event / place / attendees), the `Appointment` events-subsystem overlay, secondary facts carrying no overlay, and connector provenance; a fake-IMAP → `extract()` round-trip proving the transport and extraction compose. Calendar tests updated for the shared `RawVEvent` (`.vevent.` field access) and remain green.
- **Follow-ups:** deterministic `schema.org` JSON-LD extraction for transactional email is #249; LLM extraction for free-text prose (flights/bookings/confirmations) is C7 / #201; the Photos coords-only `took_photo` provenance-as-fact fallback is #250.
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, `README.md`, `docs/wiki/what-works-now.md`, and `Mimir-Implementation-Context.md` updated for C6.

### Connectors — Calendar review follow-up (PR #248, #198)

- **Data integrity (recurring facts):** `CalendarConnector::event_to_facts` (`mimir-connectors/src/calendar/mod.rs`) no longer sets `valid_until = DTEND` on a recurring `has_event` fact. A `RRULE:FREQ=WEEKLY` standup previously got a validity window of minutes (the first instance's `DTSTART`→`DTEND`), so current-facts reads and supersession keyed on `valid_until` treated it as long expired even though the events-subsystem overlay kept recurring. `valid_until` is now left unset when `RRULE` `FREQ` is present; a one-time event still carries its `DTEND` bound. New tests lock in both branches (`extract_one_time_event_carries_dtend_as_valid_until`, recurring `valid_until == None`).
- **Security (write-back href guard):** `CalendarConnector::act` now validates every `create_event` / `update_event` / `delete_event` `href` against the configured `calendar_url` origin (scheme + host + port + collection path) before issuing the CalDAV request, so a caller-supplied URL cannot redirect the stored Basic/Bearer credentials to another host (or an unrelated resource on the same host). An out-of-bounds `href` returns `ConnectorError::Config` with no request sent. Tests: `write_back_rejects_href_outside_calendar_origin`, `write_back_rejects_href_with_wrong_path_on_same_origin`.
- **Correctness (DST fold):** `parse_ical_datetime` (`mimir-connectors/src/calendar/caldav.rs`) now prefers the earliest offset for an ambiguous autumn-fold local time (`zone.from_local_datetime(&naive).earliest()`) before the naive-as-UTC fallback, keeping the event within an hour of the wall clock instead of shifting it by the full zone offset. A spring-forward gap still hits the fallback. Test: `parse_ical_datetime_tzid_autumn_fold_prefers_earliest_offset`.
- **Data integrity (identity trimming):** the canonical user identity is now stored trimmed at every injection site (`ConnectorContext::with_user_identity`, `ConnectorSupervisor::with_user_identity`, and the Calendar connector constructor), so a padded `[identity] name` flows through as the canonical name rather than creating a duplicate person entity. Tests: `context_with_user_identity_trims_surrounding_whitespace`, `extract_trims_padded_user_identity`.
- **Docs:** corrected the per-VEVENT fact cardinality in `docs/calendar-connector.md` (one primary + optional location + one per attendee, not "up to three"); removed stale pre-C4 status statements in `docs/wiki/calendar-connector.md`; documented the write-back href guard; aligned `docs/wiki/what-works-now.md` to 0.84.2 with 0.84.1/0.84.2 release notes; and reworded the `user_identity` doc blocks to match the actual no-identity behaviour (the primary fact is skipped, not an "Event-centric fallback").

## [0.84.1] — 2026-07-30

### Connectors — Calendar secondary-fact overlay fix (review follow-up, #198)

- **Data quality:** `CalendarConnector::event_to_facts` (`mimir-connectors/src/calendar/mod.rs`) no longer sets `valid_from`/`valid_until` on the secondary `located_in` and `attending` facts. Previously these inherited the event's `DTSTART`/`DTEND`, so for a future-dated event `event_from_extraction` (`mimir-knowledge/src/normalize.rs`) spawned a spurious `Reminder` events-subsystem overlay keyed on each secondary fact's id — a location/attendance relationship is not an event, and the module doc states only the primary `has_event` fact should drive the overlay. The secondary facts now carry no temporal bounds, so they spawn no overlay; the primary `has_event` fact (typed `EventType::Appointment`) remains the sole overlay source. `calendar_fact` now takes `valid_from: Option<DateTime<Utc>>` to express this. New assertions in `calendar_sync_surfaces_upcoming_event_for_user` (no overlay on the `located_in` fact) and `extract_emits_event_location_attendee_facts_with_identity` (secondary facts have `valid_from`/`valid_until = None`) lock in the behaviour.

## [0.84.0] — 2026-07-30

### Connectors — Calendar event extraction + write-back (Phase 3 C4 / #198)

- **Event → KB fact extraction:** `CalendarConnector::extract` (`mimir-connectors/src/calendar/mod.rs`) now drains staged VEVENTs into a cluster of `NormalizedFact`s through the shared `normalize_and_insert` pipeline. Per VEVENT it emits a primary `user has_event <event>` (typed `EventType::Appointment`, recurrence mapped from `RRULE` `FREQ`), `<event> located_in <place>`, and `<attendee> attending <event>`; every subject/object resolves to an entity via the full F5 chain (exact → alias → FTS5 fuzzy → create), and future-dated/recurring events surface in the user's "Upcoming" memory section (#74). The event entity is named by the `SUMMARY` (fallback `UID`); locations resolve to `Place` entities and attendees to `Person` entities (the `CN` parameter, else the `mailto:` value).
- **User identity injection:** `ConnectorContext` gains a `user_identity: Option<String>` field (`with_user_identity` builder) and `ConnectorSupervisor::with_user_identity` injects the canonical `config.toml` `[identity] name` so the connector authors `user has_event <event>` against the same entity the daemon resolves as `user_entity_id` (and the event surfaces in the user-scoped Upcoming section). When no identity is configured the primary fact is skipped; location/attendee facts are still emitted. This supersedes the Photos connector's disconnected per-instance `owner_name` (aligning it is tracked as a follow-up).
- **`event_type` hint on `NormalizedFact`:** `mimir-knowledge::normalize::NormalizedFact` gains `event_type: Option<EventType>`; `event_from_extraction` honours it when present, falling back to the existing `Task`/`Reminder` derivation for chat (behaviour unchanged). The Calendar connector sets `Appointment`.
- **iCalendar date + recurrence parsing:** `parse_icalendar` (`mimir-connectors/src/calendar/caldav.rs`) now parses `DTSTART`/`DTEND` to UTC at staging time — UTC (`…Z`), floating local, date-only, and `TZID`-qualified values (resolved via the new `chrono-tz` 0.10 dependency; an unknown zone falls back to the naive value as UTC so a bad `TZID` never drops the event) — and captures attendees/organizer. A new `rrule_to_recurrence` maps `RRULE` `FREQ` to the coarse `RecurrenceType` (full RFC 5545 `COUNT`/`UNTIL`/`INTERVAL`/`BYxxx` out of scope). `RawCalDavEvent` is reshaped: typed `starts_at`/`ends_at: Option<DateTime<Utc>>` replace the raw `dtstart`/`dtend` strings, plus `attendees: Vec<String>` and `organizer: Option<String>`. The now-unused `raw_ical` payload field is dropped (C4's extractor works from the parsed fields, so retaining the full calendar text per staged event was dead weight).
- **CalDAV write-back:** `CalDavClient` gains `put_event` (RFC 4791 §5.5, `If-None-Match: *` for create / `If-Match: <etag>` for update) and `delete_event` (§5.6, idempotent on 404). `CalendarConnector::act` implements `create_event` / `update_event` / `delete_event`, building VEVENTs with the `icalendar` builder and a `uuid`-generated `UID`. This is the only connector with write support.
- **Dependencies:** `chrono-tz` 0.10 and `uuid` 1 (`v4`) added to `mimir-connectors` under the `calendar` feature. `uuid` is already in the tree transitively; `chrono-tz` is a new download.
- **Tests:** new caldav unit tests (iCalendar datetime UTC/date-only/floating/TZID-DST resolution, attendee/organizer extraction) and integration tests (extraction fact shape with/without identity, recurrence mapping, write-back create/update/delete against a mock CalDAV server, idempotent 404 delete, unsupported-action error, and a full sync → `normalize_and_insert` → "Upcoming" round-trip proving the `Appointment` overlay). Existing C3 tests updated for the reshaped `RawCalDavEvent`.
- **Docs:** `docs/calendar-connector.md`, `docs/wiki/calendar-connector.md`, `docs/wiki/what-works-now.md`, `docs/wiki/events-and-reminders.md`, `README.md`, and `Mimir-Implementation-Context.md` updated for C4.

## [0.83.3] — 2026-07-30


### Connectors (review follow-up, PR #244)

- **Security (token endpoint):** `oauth::refresh_token` loopback detection (`mimir-connectors/src/oauth.rs`) now strips the surrounding `[...]` brackets that `Url::host_str` adds to IPv6 hosts before parsing them as `std::net::IpAddr`, so an `http://[::1]:<port>/token` loopback endpoint is correctly accepted instead of being over-rejected as non-loopback. The detection was extracted into a unit-testable `is_loopback_url` helper; new tests `is_loopback_url_accepts_ipv6_loopback`, `is_loopback_url_rejects_lookalike_and_remote_hosts`, and `refresh_token_accepts_ipv6_loopback_endpoint` cover the regression, while the existing `refresh_token_rejects_lookalike_loopback_host` test confirms `127.0.0.1.evil.com` stays rejected.
- **Docs:** the `0.83.2` CHANGELOG `Code quality` bullet for `EmailConnector::resolve_auth` was reworded to Rust-accurate language (a `String` has no falsy state; the behaviour is explicit handling of a missing `access_token`). `docs/wiki/what-works-now.md` version field aligned to `0.83.3`.

## [0.83.2] — 2026-07-28

### Connectors (review follow-up, PR #244)

- **Data integrity:** `ImapConnector::examine` (`mimir-connectors/src/email/imap.rs`) now returns a `ConnectorError::Parse` when the server omits the `UIDVALIDITY` response code instead of collapsing to epoch `0`, which could collide with a persisted `0:<uid>` cursor and silently skip mail (RFC 3501 mandates the response code). New test `missing_uidvalidity_is_an_error_not_zero` (fake-IMAP server gains an `omit_uid_validity` flag).
- **Stability:** `EmailConnector::use_idle` now returns `Result<bool, ConnectorError>` and the `Auto` mode defaults to IDLE (`true`) when unprobed, matching `Connector::mode` (which reports `Push` for `Auto` + `None`); previously `use_idle` defaulted to `Polling` (`false`), so a pre-probe `Auto` connector reported `Push` but polled immediately, busy-looping the supervisor. `Idle` mode now errors with `ConnectorError::Config` when the cached capability confirms the server lacks `IDLE`, honouring the documented "error if the server lacks the capability" contract. New tests `forced_idle_errors_when_server_lacks_idle` and `auto_mode_defaults_to_push_when_unprobed_matching_mode`.
- **Security (token endpoint):** `oauth::refresh_token` (`mimir-connectors/src/oauth.rs`) now rejects non-HTTPS token endpoints before posting the `refresh_token` / `client_secret`, allowing loopback HTTP only (`127.0.0.1` / `::1` / `localhost` — Mimir's local trust boundary, where credentials never traverse a network). New tests `refresh_token_rejects_non_https_endpoint` and `refresh_token_rejects_unparseable_endpoint`.
- **Security (error hygiene):** provider-supplied `error_description` in OAuth token-error responses is now truncated to 256 bytes (on a UTF-8 boundary, marked with an ellipsis) so an unbounded provider payload cannot bloat logs / the persisted `last_error`; the sanitisation promise ("only parsed `error`/`error_description`, never the raw body") is now size-bounded. New tests `truncate_description_*` and `token_error_message_truncates_a_long_error_description`.
- **Code quality:** `EmailConnector::resolve_auth` OAuth-refresh missing-`access_token` handling simplified to `ok_or_else` (matching `CalendarConnector::resolve_auth`), now handling a missing `access_token` explicitly via `ok_or_else` instead of the prior `unwrap_or_else(String::new)` + `is_empty` check (a Rust `String` has no falsy state, so the old wording overstated what `is_empty` detected). Fixed broken intra-doc link `ImmapAuth` → `ImapAuth` (`email/imap.rs`) and replaced the private `oauth` module's `[`oauth`]` intra-doc link in the crate-level docs with a code span to avoid `private_intra_doc_links`.
- **Docs:** `docs/email-connector.md` no longer documents `client_secret` in the `config_json` credential description (public-client PKCE only; confidential-client credentials never leave the `SecretBundle::OAuth` boundary). `docs/wiki/connectors.md` counts three concrete backends, drops completed IMAP ingestion from the planned section (only C6/C7 extraction remains), and places the `## How connector credentials are stored` heading on its own line so the in-page anchor resolves. `docs/wiki/email-connector.md` qualifies "Forget everything from Gmail" as a library-level capability. `docs/wiki/what-works-now.md` version field aligned to `0.83.2`. Newly touched Markdown prose (CHANGELOG `0.83.0` bullets, email-connector status/C5-C7 blockquotes, what-works-now release summary) reflowed onto single physical lines per the repo-wide prose rule.

## [0.83.1] — 2026-07-28

### Docs

- Remove duplicated doc-comment block on `EmailConnector::supports_idle` field (`mimir-connectors/src/email/mod.rs`). Code-review finding: the field comment contained a stale copy of its own first two lines, which has been collapsed into a single, accurate comment.

## [0.83.0] — 2026-07-28

### Connectors (Phase 3 C5 / #199)

- **New backend — IMAP Email connector** (`mimir-connectors`, feature `gmail`): the third concrete connector backend (after Photos and Calendar). An `async-imap` 0.11.3 client (built `default-features = false, runtime-tokio` to avoid pulling `async-std`) speaks IMAP over a hand-rolled TCP + `tokio-rustls` handshake — the workspace keeps a single rustls TLS stack instead of async-imap's `connect()` / `async-native-tls`. Login is `LOGIN` (app password) or `AUTHENTICATE XOAUTH2` (Google / Microsoft OAuth, with the SASL initial response `base64("user=<u>\x01auth=Bearer <t>\x01\x01")`).
- **Push + polling:** runs in `Push` (IMAP IDLE) mode when the server advertises `IDLE`, falling back to `Polling` otherwise — auto-detected via a `CAPABILITY` probe in `authenticate`/`health` (so `Connector::mode` returns the right value, called by the supervisor after `authenticate`). The `mode` config (`auto` | `idle` | `poll`, default `auto`) can force one.
- **Incremental sync:** `UID FETCH <last+1>:* (UID INTERNALDATE BODY.PEEK[])` with a UIDVALIDITY-safe `<uid_validity>:<last_uid>` cursor — a UIDVALIDITY mismatch on `EXAMINE` (mailbox recreated) triggers a full re-fetch, so a bare last-UID never silently gaps or duplicates. `BODY.PEEK[]` keeps mail unread. The cursor persists across restarts via the supervisor's `update_sync_cursor`.
- **Transport-only:** `extract()` stages raw RFC 822 messages and returns no `NormalizedFact`s yet. Mail parsing + structured fact extraction (headers/dates/contacts) is C6 (#200); LLM extraction (flights/bookings) is C7 (#201).
- **DRY OAuth refresh:** the Calendar connector's hand-rolled OAuth token-refresh + secret-safe error reporting moved into a shared `mimir-connectors::oauth` module; both the Calendar and Email OAuth connectors now share one implementation (avoids the reqwest-0.12-duplicating `oauth2` crate). No behaviour change to the Calendar connector; its refresh/error unit tests moved with the code.
- **Spec corrections vs. issue #199:** `async-imap 0.11.3` (not 0.11.2); rustls (not async-native-tls); UIDVALIDITY-encoded cursor (not bare last-UID). See `docs/email-connector.md`.
- **Tests:** unit tests for config/cursor/mode/auth resolution plus a fake-IMAP integration suite over a `tokio::io::duplex` pair (no TLS, no live account) covering app-password + XOAUTH2 login, IDLE push → fetch, IDLE timeout, polling incremental / no-op / full sync, and UIDVALIDITY reset. All deps already in the tree via reqwest/async-imap — no new downloads.

### Process

- **Markdown prose formatting rule:** `AGENTS.md` (Finishing Work) now requires flowing single-line prose in all repo `.md` files (README, CHANGELOG, docs/, docs/wiki/, …) and in PR descriptions and commit messages — no manual hard-wrapping with newlines inside paragraphs or list items (single newlines render as soft breaks and make raw text and diffs hard to read). A repo-wide retroactive reflow of the remaining hard-wrapped docs is tracked in #245.

## [0.82.1] — 2026-07-28 — 2026-07-28

### Docs

- **Review follow-up (PR #242):** fixed two documentation punctuation/clarity nits flagged by CodeRabbit review. `docs/calendar-connector.md`: added the missing comma before "so" and removed the unnecessary comma before "because" in the OAuth error-handling paragraph. `docs/wiki/what-works-now.md`: capitalised the sentence boundary ("The connector secret store …") in the Phase 3 roadmap summary. No code or behaviour change.

## [0.82.0] — 2026-07-27

### Connectors (Phase 3 C3 / #197)

- **New backend — CalDAV Calendar connector** (`mimir-connectors`, feature `calendar`, `Polling` mode): the second concrete connector backend. A `CalDavClient` speaks CalDAV over the existing `reqwest` 0.13 — PROPFIND (Depth 0 `resourcetype`) for calendar/health verification and a `sync-collection` REPORT (RFC 6578; root element in the `DAV:` namespace per §3.1, `calendar-data` in the CalDAV namespace) for event sync, requesting `<cal:calendar-data/>` inline so changed VEVENTs and a new `sync-token` arrive in one round trip. Omitting the sync-token does a full sync and yields the initial token; including it does an incremental sync (no full re-fetch), so the persisted sync-token is the connector's incremental cursor. `icalendar` parses each VEVENT (UID/summary/DTSTART/DTEND/location/status/ RRULE) into a staged `RawCalDavEvent`; `roxmltree` parses the WebDAV XML by local tag name (namespace-prefix tolerant).
- **Auth:** app password (HTTP Basic — iCloud/Fastmail/Nextcloud) or an OAuth bearer token (Google) that the connector **refreshes** when expired (within a 60 s skew) and persists back to the `SecretStore`. The interactive PKCE login that *obtains* the first OAuth token is deferred to A4 / #205; #197 only consumes + refreshes a stored token.
- **Framework:** this is the first backend that needs credentials, so `ConnectorContext` gained a `secret_store: Option<Arc<dyn SecretStore>>` field and `ConnectorSupervisor::with_secret_store(store)` (a breaking internal construction-context change, allowed by the project's breaking-changes policy). `ConnectorContext::with_secret_store` / `with_geocoder` builders added; `SecretStore` gained a `Debug` superbound (consistent with `Geocoder`).
- **Boundary:** `extract()` returns no facts yet — C3 is transport-only. C4 / #198 implements event → KB fact extraction + events-subsystem (#74) integration + write-back (`act`). Write-back is intentionally not in #197.
- **Dependencies:** `icalendar = "0.17"` (resolves to 0.17.6 under the workspace MSRV 1.85; 0.17.12 requires Rust 1.88 — see follow-up issue) + `roxmltree = "0.21"`, both gated by `calendar`. The `form` feature was added to the workspace `reqwest` for the OAuth refresh token POST. The `oauth2` crate is **deliberately not** pulled in (it depends on `reqwest` 0.12, duplicating the stack, and #197 only needs the refresh grant) — deferred to A4 / #205.
- **Tests:** unit tests for the CalDAV transport (sync-collection full/ incremental parse, 401 handling, PROPFIND resourcetype detection, icalendar field extraction + recurrence, invalid-payload resilience) against a `wiremock` mock server, plus integration tests (app-password sync, incremental sync-token, `full`-sync cursor reset, OAuth refresh-on-expiry + bundle persistence, health states, factory construction, config round-trip, and a full `ConnectorSupervisor` round-trip asserting the cursor is persisted). No `unsafe`.
- **Review-driven hardening (PR #242):** the `sync-collection` request now carries the required `<d:sync-level>1</d:sync-level>` element (RFC 6578 §6.3) and handles truncated (`507`) responses by paging with the advancing sync-token. A `<response>` with no `calendar-data` is tombstoned **only** on an explicit `404`/`410`; `403`/`423`/`507`/… are logged and skipped so a transient error never purges a live event. The XML helper concatenates all direct text/CDATA children (not just the first). OAuth hardening: an unknown `expires_at` no longer forces a refresh every cycle; `into_bundle` retains the prior refresh token when the response omits one; token-endpoint error bodies and the OAuth `client_secret` are never surfaced into persisted/logged `ConnectorError` strings (parsed `error`/`error_description` only; auth-kind discriminant used instead of `Debug`). `SecretBundle` gains a redacted `Debug` impl so printing a store/context never emits plaintext credentials.

## [0.81.2] — 2026-07-26


### Connectors / Knowledge (Phase 3 C2 / #196 review follow-up)

- **Stability:** `PhotosConnector::extract` now bounds geocode retries to one attempt per GPS bucket per `extract()` cycle via a per-cycle failed-key set. A sustained geocoder outage previously re-ran the geocoder's internal retry/backoff once per photo (not per distinct spot), stalling sync at ~1 req/s; now it degrades quickly to the coords-only fallback. The set is local to one cycle, so the next sync retries afresh — only success/no-match outcomes persist in the long-lived coord-dedup cache. New unit test `extract_bounds_geocode_retries_to_one_per_spot_per_cycle`.
- **Data integrity:** the single-`Geographic`-row-per-place invariant is now enforced at the schema level by a partial unique index (`idx_entity_locations_geographic_unique`, migration `047`) on `entity_id` scoped to `location_type_id = 6`. `ensure_place_coordinates` is a single atomic `INSERT ... ON CONFLICT DO UPDATE` against the index, so the read-then-write no longer relies solely on the serial overlay-worker convention. The index is deliberately partial — `Visited`/`Home`/`Work`/ `Origin`/`Current` rows are not unique per `(entity_id, location_type_id)`. New integration test `ensure_place_coordinates_keeps_single_geographic_row` (sequential + concurrent). A `const_assert` locks `LocationType::Geographic == 6` since the SQL hardcodes the literal.
- **Observability:** `normalize_and_insert` now logs (debug) when a place is created but not anchored because no coordinates resolved, instead of silently no-op'ing.
- **Docs:** MD022 blank lines around anchored headings in `docs/photos-connector.md`; renamed the stale "What's next" heading in `docs/wiki/photos-connector.md` to "Location enrichment"; documented the per-cycle retry bound and the schema-level place-anchor invariant in `docs/photos-connector.md` and `docs/entity-locations.md`.

## [0.81.1] — 2026-07-26

### Connectors (Phase 3 C2 follow-up / #196)

- **Fix:** `PhotosConnector::extract` no longer holds the staged-photo buffer mutex across the per-photo reverse-geocode network awaits. The buffer is `std::mem::take`-drained into a local `Vec` under the lock and the guard is dropped before the geocode loop, restoring the C1 lock hold-time (in-memory map only) so `forget`/`reset` or a concurrent admin-triggered sync is not blocked for the ~N-second scan duration. No semantic change; existing unit and integration tests pass unchanged.

## [0.81.0] — 2026-07-26

### Connectors (Phase 3 C2 / #196)

- **Photos GPS → place extraction:** the local-filesystem Photos connector now reverse-geocodes each photo's EXIF GPS into a locality-level place name via the shared `Geocoder`, emitting `owner took_photo_at <place>` facts whose place is a `Place` object entity. Photos at the same place corroborate into one open-ended fact (+0.05/source, capped 0.95; base confidence 0.80), so the knowledge graph grows with distinct places visited, not photo count. A coord-dedup cache (~111 m buckets) bounds geocode calls to one per shooting spot; transient network errors aren't cached. When no place resolves (no geocoder / no match / transient error), the photo degrades to the C1 coords-only `took_photo <rel_path>` shape so no data is lost.
- **Geocoder injection:** a new `ConnectorContext` (shared-services struct) is threaded factory → registry → supervisor. `ConnectorFactory::create` now receives `&ConnectorContext`; `ConnectorRegistry::create_with_context` forwards it; `ConnectorSupervisor::with_geocoder` sets the context's geocoder. No new dependencies (reuses the S1 `Geocoder`).
- **Place-coordinate anchoring:** two `entity_locations` rows per place fact — the owner's `Visited` row (coords + place name) and a new idempotent `Geographic` row (`LocationType::Geographic = 6`, migration `046`) anchoring the place entity's own coordinates, so `find_nearby` resolves places by where they are. The overlay worker derives `place_anchor` when a fact's object is a `Place` entity.
- **Geocoder:** `GeocodeResult` gained a `short_name` field (the most specific locality: city → town → village → hamlet → municipality → county → state → region, else the first `display_name` segment). The `Geocoder` trait now requires `Debug` (so `ConnectorContext` can derive `Debug`).
- **Refactor:** the location-overlay worker's `OverlayJob::Apply` payload moved into a `LocationOverlayApply` struct (fixes a clippy `too_many_arguments` lint introduced by the `place_anchor` field; keeps the worker function's argument list small).
- **Tests:** geocoder `short_name` unit + Nominatim integration tests; Photos `resolve_place` unit tests (mock geocoder place fact, no-geocoder fallback, cache hit + miss-then-hit, transient-error-not-cached) + `place_fact` / `coords_only_fact` shape tests; integration `supervisor_ingests_photo_as_took_photo_at_place_fact`; normalize `photos_at_same_place_corroborate_and_anchor_place_coords`; updated `lookup_sync_test`, `enum_roundtrip_test`, `migrations_test`, `entity_locations_test` for the new enum/count.
- **Docs:** `docs/photos-connector.md` (C2 rewrite), `docs/entity-locations.md`, `docs/geocoder.md`, `docs/wiki/photos-connector.md`, `docs/wiki/entity-locations.md`, `docs/wiki/geocoding.md`, `docs/wiki/what-works-now.md`, `README.md`, `Mimir-Implementation-Context.md`, and the `photos.rs` module header.

## [0.80.0] — 2026-07-26

### Connectors (Phase 3 C1 / #195)

- **First concrete connector backend:** the local-filesystem **Photos** connector in `mimir-connectors` (feature `photos`). A read-only, push-mode, no-network connector that watches a configured directory recursively with `notify` (debounced ~2s), extracts EXIF GPS + datetime with `kamadak-exif` (JPEG/TIFF/HEIF/PNG/WebP), and emits one `took_photo` fact per photo through the shared `normalize_and_insert` pipeline. GPS becomes a `Visited` `entity_locations` row for the owner; a per-file mtime/inode incremental cursor skips unchanged photos across restarts. Files without GPS still record a timestamped fact; files without EXIF fall back to the file mtime. C2 (#196) will reverse-geocode the coordinates into a place name.
- **Supervisor:** inject the persisted `sync_cursor` into a connector's `config_json` as `__cursor` so incremental connectors can read their prior progress (the read side that complements `KnowledgeGraph::update_sync_cursor`).
- **Dependencies:** `notify` 8.2, `notify-debouncer-full` 0.7, `kamadak-exif` 0.6 — all optional, gated by the `photos` feature.
- **Tests:** 17 unit tests (cursor diffing, EXIF parsing against committed fixtures, fact conversion, config) + 9 integration tests (initial scan, the live `notify` push watcher, incremental restart skip, the full supervisor → knowledge-graph path with a `Visited` GPS location row).
- **Docs:** `docs/photos-connector.md` + `docs/wiki/photos-connector.md`; updated `docs/connectors-framework.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md`.

## [0.79.2] — 2026-07-26

### Docs

- **README:** rework the Architecture section. The connectors entry had grown into a single ~4000-character run-on paragraph packed with issue references; split it into a crate list and a grouped key-subsystems list (knowledge graph, learning, retrieval agent, events & reminders, connectors, entity locations) so the page is scannable. Also tidy the Acknowledgments spacing.


## [0.79.1] — 2026-07-26

### Review fixes (PR #229)

- **`docs/entity-locations.md`:** add a stable `#proximity-query` anchor so the facade-API link resolves to the (Phase-3-suffixed) section heading.
- **`docs/wiki/entity-locations.md`:** drop the obsolete "later release" wording for proximity queries (now available in v0.79.0) and document the required `at` parameter in the `find_nearby` signature.
- **Migration `045` (`mimir-knowledge`):** correct the NULL-indexing rationale — a regular SQLite b-tree index includes NULLs — and switch the composite coordinate index to a partial index (`WHERE latitude IS NOT NULL AND longitude IS NOT NULL`) so it covers only the geocoded rows `find_nearby` can match and stays smaller.


## [0.79.0] — 2026-07-24

### Entity-locations proximity query (Phase 3 S4 / #194)

- **`KnowledgeGraph::find_nearby(lat, lon, radius_km, at)`** returns every `entity_location` within a radius of a point, sorted nearest-first, as `Vec<NearbyLocation>` (each entry carries the row and its exact great-circle `distance_km`). Closes the query half of #65 (write half in #193).
- **Two-stage query:** a coarse SQLite bounding-box pre-filter (`latitude`/`longitude BETWEEN ? AND ?`, now backed by a composite `idx_entity_locations_coords(latitude, longitude)` index — migration `045`) is followed by an exact Haversine post-filter computed in pure Rust that drops edge-of-box over-inclusions and sorts the survivors. NULL-coordinate (address-only) locations are skipped.
- **Temporal scoping:** `at: Option<DateTime<Utc>>` restricts to locations whose `valid_from`/`valid_until` window contains the instant; `None` is a pure spatial query over all locations.
- **Pure helpers:** `mimir-knowledge::geo` adds `haversine_km` and `bounding_box` (sphere model, mean radius 6371.0088 km, `unsafe`-free, allocation-free, unit-tested and benchmarked). No external `geo` crate — the formula is small and a heavy dependency for one function would violate the minimal-dependency stance.
- Docs: updated `docs/entity-locations.md`, `docs/wiki/entity-locations.md`, `docs/wiki/what-works-now.md`, and `README.md`.

## [0.78.2] — 2026-07-24

### Review fixes (PR #225)

- **Shutdown now drains pending location-overlay jobs.** `AppState::shutdown` calls `KnowledgeGraph::flush_location_overlays().await` after stopping the background scheduler, so queued `entity_locations` upserts complete before resources are torn down (previously a shutdown with queued overlays could drop the worker before upserting while the source fact remained).
- Docs: removed duplicate `# Changelog` heading (markdownlint MD024); scoped the single-worker throughput claim to the default Nominatim backend; fixed grammar in the entity-locations wiki.

## [0.78.1] — 2026-07-24

### Entity-locations write path fixes (Phase 3 S3 / #193)

Two correctness/performance fixes to the entity-locations overlay landed in 0.78.0, surfaced by code review:

- **Overlay now uses the inserted fact's temporal bounds.** `process_normalized_fact` destructured `valid_from`/`valid_until` once from the extracted fact and never updated them, but `handle_correction` mutates `new_fact.valid_from` before the insert (a correction scope of `None` becomes `now`, a datetime scope becomes that datetime). The location overlay now reads `fact.valid_from`/ `fact.valid_until` from the *inserted* fact, so the derived `entity_locations` row matches its source fact and prior-location supersession fires correctly for corrections (e.g. "actually I live at Y now" closes the prior open Home instead of inserting a timeless row alongside it).
- **Location overlays are offloaded to a background worker.** `apply_location_overlay` was awaited inline inside `normalize_and_insert`'s serial batch loop, so a connector batch of location facts was gated on the geocoder's rate limit (~1 req/sec for Nominatim). The geocode + upsert is now enqueued to a single background worker (FIFO `mpsc` channel) so the ingestion pipeline returns immediately and is not stalled by geocoding; the worker processes jobs in submission order, preserving move/supersession semantics both within a batch and across batches. A single worker loses no geocode throughput versus parallelism with the default Nominatim backend, which is already rate-limited to ~1 req/sec; a custom or self-hosted `Geocoder` with higher throughput could make the single FIFO worker a bottleneck. `KnowledgeGraph::flush_location_overlays` awaits every overlay enqueued before the call for deterministic shutdown / tests.
- **DRY.** The pool-based supersession upsert is extracted into `queries::entity::upsert_location`, shared by the `KnowledgeGraph::upsert_location` facade and the background worker.

## [0.78.0] — 2026-07-23

### Entity-locations write path (Phase 3 S3 / #193)

Persist structured locations (address + lat/lng + timezone) for an entity with temporal validity windows, wired into the shared `normalize_and_insert` extraction pipeline. Supersedes the write-path half of #65; proximity queries (`find_nearby`) remain a separate issue (#196).

- **Typed location overlay.** A "where" fact carries an optional `NormalizedLocation` (`location_type`, `address`, `latitude`, `longitude`, `timezone`) on `NormalizedFact`. After a non-sensitive fact is inserted, `apply_location_overlay` derives an `entity_locations` row for the resolved subject entity, using the fact's `valid_from`/`valid_until` as the location's bounds. Both the conversational `remember` path and connectors fill the same field. `NormalizedFact` is `PartialEq`-only now (`f64` coords are not `Eq`).
- **Geocode the missing half.** Via the injected `Geocoder` (stored on `KnowledgeGraph` as `Option<Arc<dyn Geocoder>>`): address-only → forward geocode to coords; coords-only → reverse geocode to a place name; both present → stored as-is. Geocoder errors/no-match are logged and tolerated; the pipeline never aborts on a geocode failure.
- **Moves / supersession.** `KnowledgeGraph::upsert_location` closes any still-open location of the same `entity_id` + `location_type` whose `valid_from` precedes the new `valid_from` (sets `valid_until`), then inserts the new row — modelling "home 2020–2023, home 2023–present". Atomic in one transaction.
- **Provenance link.** Migration `044` adds a nullable `entity_locations.source_fact_id INTEGER REFERENCES facts(id) ON DELETE SET NULL`, mirroring `events.fact_id`, so a location row traces to its originating fact and survives the fact being forgotten (FK → `NULL`).
- **Daemon wiring.** `AppState::from_config_with_llm` injects the default `NominatimGeocoder` into the `KnowledgeGraph` at startup (cheap; no network work until a location fact is processed); construction failure disables geocoding rather than aborting start.
- **LLM schema.** The `remember` tool schema gained an optional `location` object (`location_type` enum Home/Work/Visited/Origin/Current, address, latitude, longitude, timezone); `extract.rs` validates it into the overlay.
- **Mock connector.** `MockFactConfig` gained an optional `location` field so the connector → location path is exercisable.
- **No `confidence` column.** Locations do not carry their own confidence in V1; provenance is via `source_fact_id` and the source fact's confidence.
- **Pending path deferred.** Sensitive "where" facts land as `pending_confirmation`; the overlay is not applied until confirmation (follow-up).
- **Docs.** `docs/entity-locations.md` (technical), `docs/wiki/entity-locations.md` (user-facing), updated `docs/knowledge-graph-schema.md`, `docs/wiki/what-works-now.md`, and `README.md`.

## [0.77.0] — 2026-07-23

### Geocoder service (Phase 3 S1 / #191)

A pluggable geocoding abstraction with an OSM Nominatim default backend: forward geocoding (address / place name → coordinates) and reverse geocoding (latitude / longitude → place).

- **Trait + types in `mimir-core`.** `Geocoder` (async, object-safe), `GeocodeResult` (lat/lon/display_name/country/`country_code`/alternative names), and `GeocodeError` live in `mimir-core` so the Location Search tool (#98, a `mimir-core` tool) can name the trait without a dependency cycle (`mimir-core` cannot depend on `mimir-connectors`). The issue's "lives in `mimir-connectors`" wording is treated as referring to the backend, not the abstraction.
- **`NominatimGeocoder` backend in `mimir-connectors`.** Issues `GET /search` (forward) and `GET /reverse` (reverse) with `format=json&addressdetails=1& namedetails=1`; parses Nominatim's string `lat`/`lon` into `f64`. Reuses the F12 `RateLimiter` (`RateLimitConfig::nominatim`, ≤ 1 req/s) and `retry_with_backoff` for transient 429/502/503/504 + transport failures, honouring a server `Retry-After`; daily-quota exhaustion is non-retryable (`GeocodeError::RateLimited`).
- **Configurable.** Endpoint (default public instance; self-hosted Nominatim supported for heavy use), descriptive `User-Agent` (Nominatim policy), optional contact email, `RateLimitConfig`, `max_attempts`, and per-request timeout.
- **Result contract.** A successful no-match yields `Ok(None)`; transport / decode failures yield `Err(GeocodeError)` (logged, never a panic).
- **Always built** (not behind a feature flag), consistent with the framework core + mock connector.
- **Tests.** `mimir-core` unit tests (`MockGeocoder` incl. builder chaining, serde round-trip); `wiremock`-backed `mimir-connectors` integration tests (forward/reverse parsing, empty → `None`, 429-retry-then-success, persistent 503 → `Err`, non-retryable 404 no-retry, connection-refused → `Err(Network)`, rate-limiter throttling).

This is a library component; daemon wiring lands with the Photos connector (C2), the entity-locations write path (S3/#65), and the Location Search tool (#98).

### PR #221 review fixes

- Dropped the unused `reqwest` `query` feature (the backend builds query strings by hand) and de-duplicated the `reqwest` dependency comment block.
- `map_rate_err` now maps construction-time `RateLimitConfig` errors to `GeocodeError::Backend` instead of `GeocodeError::RateLimited`, reserving `RateLimited` for genuine quota-exhaustion at the admission path. Added a regression test.

### CodeRabbit review fixes

- Unparseable Nominatim `lat`/`lon` now surface as `GeocodeError::Parse` instead of silently becoming `(0.0, 0.0)`; `place_to_result` takes parsed `f64` coordinates and a new `parse_coord` helper maps malformed values to `Parse`. Added unit + integration regression tests.
- Replaced the hand-rolled query encoder with the vetted `percent-encoding` crate (already a transitive dependency via `reqwest`, resolved at 2.3.2); space now encodes as `%20` (Nominatim accepts it).
- Removed a broken rustdoc intra-doc link (`GeocodeResult::or_none`) from the `mimir-core` geocoder module docs.

## [0.76.0] — 2026-07-23

### Mock connector schema enum source-linking (PR #220 review)

Actions the remaining CodeRabbit review thread on the configurable mock connector (`mimir-connectors/src/mock.rs`):

- **Source-linked facts schema enums.** The `subject_type`, `object_type`, and `recurrence` JSON Schema `enum` lists (and their `default` values) are now derived from the serde representation of the canonical enum variant arrays instead of being hand-typed strings, so they can no longer silently desync from `EntityType` / `RecurrenceType` on a future rename.
- **Canonical variant arrays.** `mimir-knowledge` exposes `ENTITY_TYPES` and `RECURRENCE_TYPES` as single sources of truth for any caller that must enumerate the variants.
- **Regression guard.** A connector test asserts the schema enum arrays and defaults exactly equal the serde-serialised variants, and knowledge tests assert the const arrays enumerate every variant in discriminant order.


## [0.75.1] — 2026-07-23

### Mock connector review hardening (PR #220)

Actions CodeRabbit review feedback on the configurable mock connector (`mimir-connectors/src/mock.rs`):

- **Cancellation-safe sync tracking.** `MockSyncRecorder::enter(options)` now returns an RAII `MockSyncGuard` created before the first `.await` of `sync()`. Its `Drop` decrements the in-flight counter and records the `SyncOptions`, so peak-concurrency tracking stays balanced across injected failures, panics, and supervisor shutdown cancellation (push cadence sleep and `sync_delay`). Failed/panicked calls are no longer omitted from the recorder.
- **Strict `__ctype` validation.** `from_config` rejects non-integer, out-of-range, and unknown `__ctype` values with `ConnectorError::Config` instead of silently defaulting to Gmail or wrapping via `as i16`. The legacy Gmail default is kept only when `__ctype` is absent.
- **Non-zero `batch_size` contract.** `from_config` rejects `batch_size: 0` (which would let `sync()` succeed forever while fetching no facts) with `ConnectorError::Config`.
- **Fact schema matches the DTO.** The `facts` array item schema is now closed (`additionalProperties: false`) and declares the required fields plus the `subject_type`/`object_type`/`recurrence` enums, matching `MockFactConfig`.

## [0.75.0] — 2026-07-21

### Phase 3 — configurable mock connector test harness (PR for #190 / F13)

Replaces the placeholder `MockConnector` stub with a configurable, always-compiled test harness. The mock is driven entirely by its `config_json` and is the framework's test harness + the T1 sync→extract→insert→query vehicle.

- **Configurable behaviour**: `mode` (`polling`/`push`), `interval_ms`/`jitter_ms` cadence, canned `facts` (`MockFactConfig` DTO → `NormalizedFact` with `SourceType::Connector`), optional `batch_size` for incremental sync, a static `cursor`, configurable `health`/`auth_state`, and `fail_first`/`panic_first`/ `always_fail` injection. Missing `raw_reference` is auto-generated (`mock-<slug>-<index>`) so connector provenance is always satisfied. `MockConnector::default()` preserves the legacy no-op identity so existing trait tests keep passing.
- **Both modes**: polling paces via the supervisor interval; push self-paces via an internal `tokio::time::sleep` inside `sync()` (the supervisor aborts the task on shutdown for cancellation). F9 manual triggers remain rejected for push connectors.
- **Instance identity**: reads the supervisor-injected `__slug`/`__ctype`/ `__instance_id` to recover its identity, falling back to the legacy no-op when absent.
- **`MockSyncRecorder`**: optional shared observer (`with_recorder`) recording the `SyncOptions` each `sync()` receives and the peak in-flight concurrency, for F9-style serialization tests. Not part of the config schema or the factory path.
- **`config_schema()`**: returns a JSON Schema describing the config surface.
- **DRY consolidation**: the private `TestConnector` in the supervisor lifecycle tests was removed; every behavioural lifecycle test now drives the shared `MockConnector` (single source of truth for test connectors).
- **T1 vehicle** (`tests/mock_ingestion_e2e.rs`): the real `ConnectorSupervisor`
  + `KnowledgeGraph` ingest a mock's canned facts end-to-end in both polling and push modes, asserting KB facts + connector provenance (`SourceType::Connector`, `connector_instance_id`, `raw_reference`, `ExtractionMethod::StructuredParse`), with no real service.
- **No new dependencies** (in-memory; reuses `tokio`, `serde`, `chrono`).
- **Fix (review #220)**: the `batch_size` slice is now keyed on the *successful*-sync counter, not the raw call count, so failed/panicked cycles no longer consume a batch window and silently drop facts. Regression tests added for the `fail_first` + `batch_size` and `panic_first` + `batch_size` combinations.
- **Breaking change** to the public `MockConnector` type (unit struct → config-driven struct); acceptable per the project breaking-changes policy (only the OpenAI-compatible chat endpoint is stability-sensitive).

Documentation: `docs/mock-connector.md` (technical), `docs/wiki/mock-connector.md` (user-facing), plus updates to `docs/connectors-framework.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, and `README.md`.

## [0.74.0] — 2026-07-20

### Phase 3 — rate-limit review follow-ups (PR #219)

Addresses the remaining CodeRabbit review threads on the connector rate-limiting primitives (issue #189 / F12).

- **Overflow-safe snapshot restore**: `RateLimiter::with_quota_state` now validates a restored `QuotaSnapshot` with `DateTime::checked_add_signed` before constructing quota state, returning the new `RateLimitError::InvalidSnapshot` when `window_start + window` would overflow `DateTime<Utc>`. A crafted, `serde`-deserialisable snapshot near `DateTime::<Utc>::MAX_UTC` can no longer panic inside `is_exhausted` / `check_and_increment`.
- **Monotonic persistence protocol**: `QuotaSnapshot` gains a `version: u64` field (with `#[serde(default)]`) that increases on every successful `acquire` and is carried across reconstruction, so a persistence layer can use it as a compare-and-swap guard and never regress a window's count via delayed, out-of-order writes. `docs/connector-rate-limiting.md` documents the persist-before-dispatch and never-regress protocol.
- Tests added for `MAX_UTC` snapshot rejection, monotonic `version` across acquires and reconstruction, and backward-compatible deserialisation of pre-`version` snapshots.

## [0.73.0] — 2026-07-17

### Phase 3 — rate-limit review fixes (PR #219)

Addresses CodeRabbit review feedback on the connector rate-limiting primitives (issue #189 / F12).

- **New public API** for daily-quota persistence across restarts: `QuotaSnapshot`, `RateLimiter::with_quota_state`, and `RateLimiter::quota_snapshot`. A reconstructed limiter resumes the saved rolling 24h window instead of resetting the allowance to zero, so a daemon or connector relaunch cannot silently bypass a provider's hard 24-hour quota.
- **Fail-fast quota exhaustion** in `RateLimiter::acquire`: a known-exhausted daily quota is now reported before awaiting the token bucket, so a low-rate limiter no longer parks a task for the full replenish interval (potentially hours) before returning `QuotaExhausted`. The authoritative increment still runs after token admission.
- **Bounded jittered delay**: the retry delay is now clamped *after* jitter is applied, so a `Retry-After` at the strategy cap can no longer become `cap + jitter` and breach the bounded-delay contract (`retry_delay_with_jitter`).
- **Saturating duration arithmetic**: the `Linear` backoff and jitter paths use `Duration::saturating_add`, preventing overflow panics on config-derived values near `Duration::MAX` before the `max` clamp.
- **Documentation** corrected to describe the primitives as available infrastructure for future connector adoption, not as already wired into every connector's outbound calls.
- Tests added for snapshot round-trip/restore, fail-fast exhaustion, clamp-after-jitter, and near-`Duration::MAX` saturation; the daily-quota `resets_at` assertion now verifies an ~24h window rather than merely `> before`.

## [0.72.0] — 2026-07-17

### Phase 3 — connector rate limiting & retry (issue #189 / F12)

Shared rate-limiting + retry/backoff primitives for network connectors, in the new `mimir_connectors::rate_limit` module. The primitives are available infrastructure now; connectors will route their outbound HTTP/IMAP/CalDAV API calls through one per-instance `RateLimiter` for uniform throttling, daily-quota enforcement, and 429/503 retry as their backends land in later Phase 3 issues.

- `RateLimitConfig { requests_per_second, burst_size, daily_quota, backoff_strategy }` — `serde`-serialisable (human-readable durations via `humantime`) so it embeds in each connector's `config_json`; a `RateLimitConfig::nominatim()` preset enforces the OSM Nominatim ≤ 1 req/s usage policy.
- `RateLimiter` — token bucket backed by `governor` (a vetted, `unsafe`-free GCRA implementation) for `requests_per_second` + `burst_size`, with an optional rolling 24h daily quota. Quota exhaustion returns a non-blocking `RateLimitError::QuotaExhausted { resets_at }` so the `ConnectorSupervisor` can pause the cycle gracefully instead of parking a task for up to 24h.
- `BackoffStrategy` — exponential / linear / fixed, each with a jitter budget.
- `retry_with_backoff` — generic retry helper for transient failures, with a `Retryable` trait and `RetryHint::from_status` classifying `{429, 502, 503, 504}` (matching the `LlmClient` transient set) and honouring a server-supplied `Retry-After`, clamped to the strategy's `max` cap (or a 5-minute default for `Fixed`) so an unreasonable hint cannot stall a connector task.
- Connector **LLM** calls are exempt (decision D′): they route through the shared `LlmWorkerPool` system queue and are not wrapped by this limiter.

New dependencies (version-checked on crates.io): `governor` 0.10, `rand` 0.9 (pinned to the line `governor` already pulls in transitively), `humantime` 2.4. No `sqlx`; no `unsafe`. Unit + integration tests cover throttling, quota exhaustion/reset, backoff progression, retry success/exhaustion/terminal, `Retry-After` honouring + clamping, config serde, and presets.

## [0.71.1] — 2026-07-17

### Docs

- Corrected the `FileSecretStore` atomic-write documentation in `docs/connector-secret-store.md` to describe the actual temporary-file naming pattern (`<slug>.json.tmp.<pid>.<counter>`, using the process id and a per-process monotonic counter) rather than the placeholder `<slug>.json.tmp`, so cleanup and monitoring logic is not misled. Addresses PR #218 review feedback.

## [0.71.0] — 2026-07-17

### Phase 3 — connector secret store (issue #187 / F10)

A single `SecretStore` trait now backs every connector auth kind. One `SecretBundle` enum covers OAuth 2.0, API tokens, and app passwords, and the V1 default `FileSecretStore` persists one JSON file per connector instance under `~/.local/share/mimir/secrets/<slug>.json`, file mode `0600`, parent directory `0700`, plaintext at rest. Loads *fail closed*: a secret file or directory with any group/other permission bits set is refused (`SecretError::InsecurePermissions`) rather than read. Writes are atomic (temp file + rename) so a crash cannot truncate a secret. Slugs are validated against `[A-Za-z0-9_-]{1,128}` before touching the filesystem, blocking path-traversal. An `InMemorySecretStore` is included as a test/helper backend.

At-rest encryption is intentionally deferred (consistent with the plaintext LLM API key in `config.toml` and the home-directory trust boundary); a `keyring`-backed store is tracked separately as #188. The end-to-end `connector remove` secret wipe is the consumer's responsibility (server/CLI routes in #202/#204/#203); this issue delivers the `delete(slug)` capability.

- **New public API (mimir-connectors):** `SecretStore`, `SecretBundle`, `FileSecretStore`, `InMemorySecretStore`, `SecretError`.
- **New public API (mimir-core paths):** `secrets_dir()`, `secrets_file(slug)`.
- **No new dependencies** (uses existing `serde`/`serde_json`/`chrono`/ `async_trait`/`thiserror`/`tracing`; permission enforcement uses the std `std::os::unix::fs::PermissionsExt` safe API).
- **Design note:** `SecretBundle` uses struct variants (`ApiToken { token }`, `AppPassword { password }`) rather than newtype variants so serde's internally-tagged `kind` representation works; the on-disk JSON is self-describing. `OAuth.refresh_token` and `OAuth.expires_at` are `Option` since not all grants issue a refresh token or return an expiry.
- Framework core (not feature-gated); `--no-default-features` still compiles the secret store alongside the rest of the framework + mock.

## [0.70.0] — 2026-07-14

## [0.70.0] — 2026-07-14

### Phase 3 — manual sync triggering (issue #186 / F9)

`ConnectorSupervisor::trigger_sync(id, SyncOptions)` (and a slug-based `trigger_sync_by_slug`) wakes a connector's runner from its polling-interval wait so a sync runs immediately with caller-supplied options — `--full` forces a non-incremental pass (cursor ignored/reset) and `since` is a relative time-window hint. A one-permit `tokio::sync::Semaphore` per connector serialises concurrent callers (overlapping triggers queue rather than launching parallel cycles), and a per-connector request channel carries the options and returns the cycle's `TriggerOutcome` (`Ok { fetched, new_cursor }`, `AuthExpired`, or `Failed`). Triggering a connector that is not running (`Paused`/`Error`/`Setup` or exited) returns `TriggerError::NotRunning`; push-mode connectors (no polling interval to preempt) return `TriggerError::PushUnsupported` — push manual sync is deferred to a later Phase 3 issue.

- **New public API:** `ConnectorSupervisor::trigger_sync`, `ConnectorSupervisor::trigger_sync_by_slug`, `TriggerOutcome`, `TriggerError`, and `SyncOptions::Default` (incremental, no window).
- **Runner loop rework:** the post-cycle wait is now a `select!` between the polling interval, a manual trigger, and shutdown; a manual trigger preempts the interval. Backoff after a failed cycle is likewise preemptable by a trigger. `run_cycle` takes `SyncOptions`; `CycleOutcome::Ok` now carries the `SyncOutcome` so a triggered cycle can report stats to the caller.
- **No new dependencies** (`tokio` `sync` feature already enabled).
- This is a library component in `mimir-connectors` with integration tests against a configurable in-memory mock; daemon `AppState` wiring and the `mimir connector sync …` CLI land in later Phase 3 issues (A1–A3).

## [0.69.2] — 2026-07-13

### Review fixes for PR #216 (CodeRabbit)

- **Docs:** clarified the shutdown integration boundary in `docs/wiki/connectors.md` and `docs/wiki/what-works-now.md` — the supervisor supports the shared `watch` shutdown channel, but `mimir stop` does not yet drive it (daemon/CLI wiring is deferred to later Phase 3 issues).
- **`mimir-connectors`:** de-duplicated the `CycleResult::Err` and `CycleResult::Panic` failure handling into a shared `record_failure` async helper so the circuit-breaker / backoff policy cannot drift between the sync-error and panic paths.
- **Tests:** `none_cursor_preserves_existing_sync_cursor` now captures the seeded `last_sync_at` and waits for it to advance (not merely exist), ensuring the poll observes a completed `None`-cursor cycle rather than the pre-existing row state.

## [0.69.1] — 2026-07-13

### Bugfix — None sync cursor no longer wipes persisted progress token

`run_cycle` previously passed `SyncOutcome::new_cursor` straight into `KnowledgeGraph::update_sync_cursor`, whose `None`-clears contract wiped the persisted `sync_cursor` whenever a connector returned `new_cursor: None` (documented as "unchanged"). The next incremental sync then re-fetched from the beginning, defeating the "no re-fetch after `mimir stop`" guarantee.

- **Fix:** `run_cycle` now branches — `Some(cursor)` advances (or clears) the cursor via `update_sync_cursor`; `None` stamps `last_sync_at` only via the new `KnowledgeGraph::touch_last_sync`, preserving the progress token.
- **New KG method:** `touch_last_sync(id)` — stamps `last_sync_at` and `updated_at` without rewriting `sync_cursor`.

## [0.69.0] — 2026-07-13

### Phase 3 — ConnectorSupervisor supervised lifecycle (issue #185 / F8)

`ConnectorSupervisor` owns one supervised background task per connector whose lifecycle status is `Active`, centralising everything needed to keep a connector running safely. It is the caller that runs the two-step ingestion model end to end: `health` → `sync` → `extract` → `normalize_and_insert`, then `update_sync_cursor`. All status / auth / cursor writes go through the `KnowledgeGraph` facade — the crate stays `sqlx`-free. No concrete backends sync yet (this is a library component; daemon wiring lands in A1).

- **`SupervisorConfig`** (`max_failures`, `base_backoff`, `max_backoff`): injected at construction (no environment mutation, per the safety policy). Backoff is deterministic in V1 (`base_backoff * 2^(n-1)`, capped at `max_backoff`); randomised jitter / rate-limit primitives belong to F12.
- **`ConnectorSupervisor`**:
  - `restore()` — loads the `connectors` table and spawns a runner for every `status == Active` row. `Paused` / `Error` / `Setup` rows are left down. Rows with no registered `(type, backend)` factory or invalid `config_json` are logged and skipped (one bad connector never aborts startup).
  - `shutdown()` — aborts and joins all runner handles (defensive fallback; the shared `watch` channel normally drains them first).
  - `running_count()` / `is_running(id)` — observability for tests/wiring.
- **Per-connector runner loop.** Initial `authenticate()` handshake (a failed handshake pauses and exits). Then each cycle runs in an **isolated sub-task** (`tokio::spawn`) so a connector panic surfaces as `JoinError::is_panic` instead of unwinding the runner. The cycle and the shared shutdown signal race in a `tokio::select!`; shutdown aborts the in-flight cycle via its `AbortHandle`.
- **Lifecycle mapping.** Success resets failures, persists the cursor, clears `last_error`. Error/panic increments failures (status stays `Active` + recorded `last_error`), applies exponential backoff, and after `max_failures` consecutive failures moves the connector to `Error` (circuit breaker; stops auto-restart, manual `resume` required). `health() == AuthExpired` sets `auth_state = Expired` + `status = Paused` and exits (not auto-restarted).
- **Graceful shutdown + cursor persistence.** The supervisor observes the same shared `watch::Receiver<bool>` shutdown channel the daemon uses for OS signals and `/stop`, so `mimir stop` drains every runner. The cursor is persisted after every successful `sync`, so it always reflects the last completed sync; `mimir stop` mid-cycle aborts the in-flight cycle (no cursor advance) and the next restart resumes from the last persisted cursor.
- **Instance identity injection.** `restore` augments each row's `config_json` with `__slug`, `__ctype`, and `__instance_id` before passing it to the factory — the V1 mechanism for giving a connector instance its row identity through the minimal `create(config)` signature (the LLM/SecretStore construction context is deferred to F10 / the first real backend).
- **`yield-on-user-activity`** deferred for V1 (`last_user_activity` is not consulted yet).
- **`SupervisorError`** (`Knowledge`, `Connector`, `Json`): thiserror enum.
- **Dependencies.** `tokio` promoted from dev-only to a real dependency of `mimir-connectors` (`rt`, `sync`, `time`, `macros`); `tracing` added for structured logging. No new external crates (tokio is already pinned to `1` workspace-wide; patterns verified via Context7).
- **Tests.** New `tests/supervisor_lifecycle.rs` (7 integration tests against a real in-memory knowledge graph and a configurable test-local connector mock): startup restore (Active only), cursor persistence on shutdown, transient failures → backoff → recovery, circuit breaker, auth-expiry pausing, panic recovery, and push-mode in-flight cancellation. The shared `MockConnector` is untouched (F13 owns the real harness).


## [0.68.0] — 2026-07-08

### Phase 3 — ConnectorRegistry + multi-backend factory dispatch (issue #184 / F7)

`ConnectorRegistry` now maps `(connector_type, backend)` to a `ConnectorFactory`, enabling the multi-backend architecture: a connector *type* (Email/Calendar/Photos) is the provenance/reliability axis; a *backend* (IMAP, CalDAV, local-FS, …) is the provider implementation chosen per instance and stored as the `backend` column on `connectors` (F2). New backends register a new factory — no schema change. No concrete backends sync yet.

- **`ConnectorFactory` trait** (`Send + Sync`, object-safe): `create(config: serde_json::Value) -> Result<Arc<dyn Connector>, ConnectorError>`. Construction is synchronous and cheap; network/auth happen later via `Connector::authenticate` / `sync`. The V1 factory takes only `config`; decision D′ (`Arc<dyn LlmBackend>` at construction) and F10 (`SecretStore`) will extend the signature when F8/F10 land (acceptable internal-API break).
- **`ConnectorRegistry`** (`RwLock<HashMap<(ConnectorType, String), Arc<dyn ConnectorFactory>>>`, `&self` registration matching `ToolRegistry`): `register`/`register_arc`, `is_registered`, `factory`, `backends_for`, `registered_types`, `create`, plus `len`/`is_empty`. Duplicate `(type, backend)` registration fails loud with `BackendAlreadyRegistered`; unknown-pair `create` returns `BackendNotFound`.
- **`FnConnectorFactory`** — closure-backed factory for simple backends/tests.
- **`MockConnectorFactory`** — always-compiled factory producing `MockConnector`s, keeping the registry exercisable under every feature combination (including `--no-default-features`).
- **`ConnectorError`** — new variants `BackendNotFound` and `BackendAlreadyRegistered`.
- **`mimir-knowledge`** — `ConnectorType` now derives `Hash` so it can key the registry's `HashMap`.
- **Reliability stays per-type:** confidence remains `confidence::initial(SourceType::Connector, connector_type)`, keyed on the type axis only; the registry never branches reliability on `backend`.

### Review fixes (PR #215)

- **deps:** Removed redundant `async-trait` and `chrono` re-declarations from `mimir-connectors` `[dev-dependencies]` (both are already regular dependencies and thus available to integration tests) — DRY.
- **registry:** Poisoned-`RwLock` handling is now consistent across every accessor: all read/write acquisitions go through private `read`/`write` helpers that `.expect` on poison, matching the `ToolRegistry` convention. Previously the write-side methods surfaced poison as `ConnectorError::Other` while read-side methods silently returned empty/false/`None`, which could report contradictory state after a panic.
## [0.67.1] — 2026-07-08

### Review fixes (PR #214)

- **docs:** Corrected the `mimir-connectors` framework status summary so the mock harness is no longer described as a stub (it is now implemented and documented), keeping the phase status consistent with the `MockConnector` section.
- **tests:** The `trait_is_object_safe` test now exercises the default `act()` implementation through `Arc<dyn Connector>`, confirming it dispatches correctly through the trait object and returns `ConnectorError::UnsupportedAction`.

## [0.67.0] — 2026-07-08

### Phase 3 — Connector trait + data types (issue #183 / F6)

The runtime `Connector` trait and its supporting data types are now defined in `mimir-connectors`, replacing the F1 identity-only stub. This is the contract every service-ingestion worker implements; no concrete backends yet.

- **`Connector` trait** (`#[async_trait]`, `Send + Sync`, object-safe as `dyn Connector`): `id()` (instance slug), `name()`, `connector_type() -> ConnectorType`, `mode() -> ConnectorMode`, `config_schema() -> serde_json::Value`, `authenticate() -> ConnectorAuthState`, `health() -> HealthStatus`, `sync(SyncOptions) -> SyncOutcome`, `extract() -> Vec<NormalizedFact>`, optional `act(ConnectorAction) -> ActionResult` (default impl returns `UnsupportedAction`), and `forget()`.
- **Ingestion model (locked):** two-step and DB-free. `sync()` fetches raw items into a connector-internal buffer; `extract()` drains them into typed `NormalizedFact`s (entity *types* set, entity *ids* unresolved). The supervisor (F8) builds the `Provenance` and calls `mimir_knowledge::normalize::normalize_and_insert` to resolve entities, score confidence, gate sensitivity, and insert. The trait takes no `&KnowledgeGraph`, so connectors stay `sqlx`-free and unit-testable without a live knowledge graph.
- **New data types:** `ConnectorMode { Polling{interval,jitter} | Push }`, `SyncOptions { full, since }`, `SyncOutcome { fetched, new_cursor, fetched_at }`, `HealthStatus { Online | Offline | Degraded | AuthExpired | NotConfigured }`, `ConnectorAction`, `ActionResult`, and a `thiserror`-based `ConnectorError`.
- **`HealthStatus` is a transient runtime probe**, deliberately renamed to disambiguate from the persisted `ConnectorStatus` / `ConnectorAuthState` enums; the supervisor maps probe outcomes onto the persisted lifecycle columns.
- **Reuse (DRY):** `ConnectorType` / `ConnectorAuthState` and `NormalizedFact` are consumed directly from `mimir-knowledge`; no parallel `ExtractedFact` / `RawEvent` types are introduced.
- `MockConnector` now satisfies the full trait so the always-compiled mock path stays valid under every feature combination. 13 new behavioural tests cover the trait surface, object safety, the polling/push distinction, and the renamed health variants.

Dependencies added to `mimir-connectors` (version-checked on crates.io): `async-trait 0.1`, `serde 1.0`, `serde_json 1.0`, `chrono 0.4`, `thiserror 2.0` (+ `tokio 1` dev-dependency). No `sqlx`.

## [0.66.0] — 2026-07-08

### Phase 3 — Full entity-resolution chain (issue #182 / F5)

`resolve_entity` (`mimir-knowledge::normalize`) now runs the full resolution chain shared by chat extraction and connectors: exact name → exact alias → FTS5 fuzzy → create new, with two correctness policies layered on top of the existing `get_by_name` search:

- **Strict same-type filtering.** A new `get_by_name_typed` lookup restricts candidates to the requested `EntityType`, so a cross-type token-overlap match (e.g. "Apple" resolved as a `Concept` vs "Apple Inc" the `Organization`) is dropped and a new entity is created instead of a wrong merge. The untyped `get_by_name` remains the general-purpose search surface.
- **Fuzzy resolution gate.** A pure `pick_resolution` policy resolves exact-name and exact-alias hits unconditionally, but accepts a fuzzy hit only when its normalised score is ≥ `FUZZY_RESOLVE_THRESHOLD` (`0.9`); weaker fuzzy matches fall through to create-new. Alias creation is not auto-learned from fuzzy matches — it stays explicit via `preferred_name`.

Tests: 9 unit tests for the decision policy (threshold boundary, alias/exact precedence) and 6 integration tests through `normalize_and_insert` (alias-, fuzzy-, exact-, create-on-miss, and cross-type-create paths). No regression in the chat extraction E2E suite.

## [0.65.0] — 2026-07-08

### Phase 3 — Shared normalize/insert boundary (issue #181 / F4)

Extract the resolve → confidence → sensitivity-gate → insert orchestration from the conversational `remember` path into a reusable `normalize_and_insert(kg, Vec<NormalizedFact>, Provenance) -> ExtractionOutcome` boundary in `mimir-knowledge::normalize`. Both chat learning and (future) service connectors now funnel through one deterministic Rust pipeline.

- **New public types** in `mimir-knowledge::normalize`:
  - `NormalizedFact` — provenance-annotated, per-fact content with typed entity types, parsed temporal bounds, typed `RecurrenceType`, validated category ids, the sensitivity flag, an optional correction scope, and the per-fact `raw_reference`. `source_type` is per-fact because a chat batch may mix `Explicit`/`Casual` facts; connectors set `Connector`.
  - `Provenance` — batch-level origin: the connector instance id + type (for connector syncs) and the `extraction_method` (`LlmExtraction` for chat, `StructuredParse` for structurally-parsed connector items). Constructors `Provenance::chat` and `Provenance::connector`.
  - `ExtractionOutcome` / `PendingFact` move to `normalize` and are re-exported from `mimir_knowledge::extract` for existing callers.
- **Confidence** is `confidence::initial(source_type, connector_type)` — the per-source-type / per-connector reliability score with **no extraction-method discount**. Corroboration, supersession, and inference are inherited for free from `insert_fact_in_tx`.
- **Sensitivity** uses the same Rust `AND`-gate as conversational facts: connector facts the producer flags sensitive land as `pending_confirmation` and surface via `kb audit`.
- **Conversational refactor:** `extract.rs` keeps the LLM-call half (tool schema, prompts, output parsing) plus an `extracted_to_normalized` adapter that canonicalises predicates, splits list objects, and parses LLM string fields into typed `NormalizedFact`s. `process_remember_output`, `extract_facts`, and `extract_facts_with_context` route through `normalize_and_insert`; chat behaviour is unchanged.
- **Tests:** new `mimir-knowledge/tests/normalize_test.rs` covers a connector-produced `NormalizedFact` insert, the cross-connector corroboration acceptance criterion (Gmail flight + Calendar event → one fact, two sources, confidence boosted to the 0.95 cap), chat provenance, and the connector provenance gate. All existing extraction/optimization/confirmation tests pass unchanged.

### Tests — pre-existing `mimir-server` harness failures fixed

Six `mimir-server` lib tests that failed identically on `main` (verified in a detached worktree) were stale test-harness bugs, not production defects:

- **`insert_pending_fact` helper** (5 `test_kb_*` tests): the helper built a sensitive allergy fact with `is_sensitive: true` but **no catalogue category**. After the #142 sensitivity `AND`-gate landed, Rust correctly narrows such a fact to non-sensitive (no sensitive category and no sensitive keyword in `"peanuts"`), so it never reached `pending_confirmation` and the helper panicked indexing into an empty result. Fixed by assigning category 230 (Allergies & Intolerances), mirroring `extract.rs`'s `sensitive_allergy_fact` helper.
- **`test_non_incognito_allows_remember_tool_and_persists_fact_stream`** and the paired `test_incognito_..._stream` (which was a false pass): both hit `/chat/stream` but queued responses via the blocking-path mock API (`push_chat_message`/`push_chat`) instead of the stream-path API (`push_stream`/`StreamItem`). The stream therefore errored out on an empty queue before executing the `remember` tool. Fixed by queueing `StreamItem::ToolCalls` + a follow-up `StreamItem::Text`, and (for the non-incognito case) draining the SSE body so the spawned stream task completes fact persistence before the assertion. The incognito test now exercises the incognito write-guard for the right reason instead of passing by accident.

Production code was unchanged; only test harnesses were corrected.

### Review fix — scope-less correction regression

The boundary originally gated `handle_correction` on `correction_scope.is_some()`, which silently dropped the defensive temporal-correction-at-`now` behaviour: a conversational `Correction` fact with no `correction_scope` (which the LLM may emit despite being told to set one) was treated as an ordinary `Explicit` fact and never superseded its open-ended predecessor. Fixed by carrying an `is_correction: bool` on `NormalizedFact` (set by the chat adapter from the LLM `Correction` classification; connectors always leave it `false`) and gating on that signal instead, so the `handle_correction` `None` arm is reachable again. A regression test (`test_correction_no_scope_defaults_to_temporal_at_now`) covers the path.

### Review fix — sensitive facts dropped catalogue categories

`insert_sensitive_fact` (the pending-confirmation insert path) wrote the fact and source but skipped the `fact_categories` junction writes that the normal insert path performs, so sensitive facts lost their catalogue category links. Category-based reads and downstream memory/sensitivity logic could therefore miss them. Fixed by persisting `new_fact.category_ids` in the same transaction via `INSERT OR IGNORE INTO fact_categories`, mirroring `insert_fact_internal`. A regression test (`sensitive_fact_persists_its_catalogue_categories`) covers the path.

### Review fix — markdown lint hygiene

- `docs/wiki/what-works-now.md`: replace the blank blockquote separator (MD028, no-blanks-blockquote) between the release-summary and corroboration blocks with a plain blank line so they render as separate blockquotes.
- `docs/fact-extraction-pipeline.md`: add a blank line after the new `### Scope-less Correction (`None`)` heading (MD022, blanks-around-headings).

## [0.64.0] — 2026-07-07

### Phase 3 — Sources provenance FK migration (issue #180 / F3)

Migrate `sources.connector_id TEXT` to `connector_instance_id INTEGER REFERENCES connectors(id)`, so every fact's connector provenance points at a registered connector instance instead of a free-form string label. `connector_type_id` is retained (denormalised) so the confidence model can read the connector kind without a join.

- **Migration `043_sources_connector_instance_fk.sql`** rebuilds the `sources` table (SQLite cannot change a column type in place). It is lossless for existing DBs: legacy `sources.connector_id` values were already limited to `NULL` or `''` (the insert paths differ — `queries/source.rs` normalised a missing connector to `''`, `queries/fact.rs` bound `NULL`), and both map to `connector_instance_id IS NULL`. The NULL-aware unique index is rebuilt as `(fact_id, source_type_id, COALESCE(connector_instance_id, 0), COALESCE(raw_reference, ''))` (`0` is a safe sentinel since autoincrement ids start at `1`), plus a new `idx_sources_instance` index for item-count queries.
- **Rust model:** `Source.connector_instance_id: Option<i32>` and `NewFact.connector_instance_id: Option<i32>` replace the old `Option<String>` labels. Every insert site (`extract.rs`, the corroboration + `insert_fact_in_tx` paths in `queries/fact.rs`, `queries/trash.rs` restore, `optimization/mod.rs` dedup-merge), the `SourceInput`/`AddSourceRequest` facade, and the public `SourceRow` wire type (`connector_instance_id: Option<i32>`) were updated.
- **Validation gate:** `insert_fact` now keys connector provenance off `connector_instance_id` rather than `connector_type`. When set it requires `raw_reference` and `extraction_method`, resolves the instance, and enforces consistency — a supplied `connector_type` must match the instance's registered `connector_type_id` (else `KnowledgeError::Validation`), or it is derived from the instance when omitted. An unregistered instance id is rejected. This provenance validation always runs when `connector_instance_id` is set, independent of whether `confidence` is supplied explicitly, so an explicit confidence can no longer bypass the `raw_reference`/`extraction_method` requirement or the `connector_type` consistency check.
- **Forget filter:** `forget --source <slug>` now matches `connectors.slug` via subquery (plus the existing `source_types.name` arm), since the column is no longer a free-form string.
- **Item counts** are now derivable via `SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?`, closing the acceptance deferred from #179.

## [0.63.0] — 2026-07-07

### Phase 3 — Connector instance registry (issue #179 / F2)

The `connectors` instance-registry table and its `KnowledgeGraph` facade. Each row is a single configured connector instance (one Gmail account, one CalDAV calendar, …); connector backends persist their sync cursor, auth state, and health here so they survive daemon restarts. This unblocks the supervisor (F8) and per-connector backends (C1–C7).

- **Migration `042_create_connectors.sql`** adds the `connectors` table plus two lookup tables, `connector_statuses` (`Setup`, `Active`, `Paused`, `Error`) and `connector_auth_states` (`Unauthenticated`, `Authenticated`, `Expired`). Integer PKs and `_id` foreign keys follow project convention; `slug` is a unique human label. The `sources.connector_instance_id` provenance FK and item-count query are deferred to F3.
- **Typed enums:** `ConnectorStatus` and `ConnectorAuthState` (`#[repr(i16)]`, `sqlx::Type`, `TryFrom<i16>`) in `models/enums.rs`, with stability and serde round-trip tests. This deliberately deviates from the issue's `TEXT` schema to match the existing `EventStatus`/`FactStatus` pattern and the project's "smallest data type" rule.
- **Model + queries:** `models/connector.rs` (`Connector` row + typed accessors + `UpsertConnectorInput`) and `queries/connector.rs`. The `Connector` row stores lookup ids as raw `i16` with typed accessors, mirroring the `Event` overlay model.
- **Facade methods** on `KnowledgeGraph`: `list_connectors`, `get_connector_by_slug`, `get_connector`, `upsert_connector`, `update_sync_cursor`, `set_connector_status`, `set_auth_state`. Upsert is keyed on `slug`; `slug` and `connector_type` are immutable identity. On conflict it updates the mutable config surface (`backend`, `display_name`, `config_json`, `status`, `auth_state`) and preserves sync-progress fields (`sync_cursor`, `last_sync_at`, `last_error`); reusing a slug with a different `ConnectorType` returns `KnowledgeError::ConnectorTypeMismatch` rather than silently rewriting the instance's kind. `set_connector_status` takes an `Option<Option<String>>` `error` parameter (leave / clear / set `last_error`). New `KnowledgeError::ConnectorNotFound` (unknown id) and `ConnectorTypeMismatch` variants; the typed `ConnectorType` input makes the `connector_types` FK guaranteed valid.
- **Tests:** `tests/connectors_test.rs` covers defaults, slug/id/list lookup, upsert update-vs-preserve, connector-type mismatch rejection, sync cursor, status set/clear/leave, auth state, duplicate-slug upsert, and missing-id errors; `tests/migrations_test.rs` asserts the new tables exist and are seeded.
- **Docs:** updated `docs/connectors-framework.md`, `docs/wiki/connectors.md`, `README.md`, `Mimir-Implementation-Context.md`, and `docs/wiki/what-works-now.md`.

## [0.62.1] — 2026-07-02

### Bugfix — shutdown trigger attribution logging

The daemon logged only `Server shut down gracefully.` for **every** shutdown path, so the *cause* of a stop was unobservable from the journal. An unexplained stop on 2026-06-30 (systemd recorded no `Stopping`/`Stopped` lifecycle line, proving the trigger originated inside the process) could not be attributed from logs.

- **`ShutdownSource` enum** (`mimir-server/src/lib.rs`) classifies the origin of a shutdown: `StopEndpoint(SocketAddr)`, `Terminate` (SIGTERM), or `Interrupt` (Ctrl-C / SIGINT). Each trigger path now logs `ShutdownSource::attribution()` *before* firing the shared `shutdown_tx` watch trigger, e.g. `Shutdown requested via /stop endpoint from 127.0.0.1:45678.`
- **`/stop` handler** (`mimir-server/src/routes/stop.rs`) now captures the requesting peer via an axum `ConnectInfo<SocketAddr>` extractor and logs it (loopback-guaranteed by the existing `require_loopback` middleware).
- **OS-signal listener** (`spawn_os_signal_shutdown`) now distinguishes which signal fired (SIGTERM vs Ctrl-C) and logs the matching attribution.
- **Untriggered exits are no longer mislabelled.** `serve_with_bounded_drain` previously logged `Server shut down gracefully.` even when the server future resolved on its own with no shutdown trigger (Phase 1). This is now reported at `warn!` level as `Server future resolved without a shutdown trigger; exiting.` via the pure `server_exit_message(triggered: bool)` helper, so a non-graceful exit can no longer masquerade as graceful.
- **No public API change.** `ShutdownSource` and `server_exit_message` are `pub(crate)`; the `/stop` route behaviour is unchanged.
- **Tests:** `test_shutdown_source_attribution_messages`, `test_server_exit_message_distinguishes_untriggered_exit`, and `test_stop_handler_fires_shutdown_trigger` (TDD).
- **Docs:** updated `docs/shutdown.md` and `docs/wiki/daemon-shutdown.md`.


## [0.62.0] — 2026-07-02

### Phase 3 — Connectors framework scaffold (issue #178 / F1)

- **New crate:** `mimir-connectors` — the service ingestion framework for Mimir. Connectors are background sync workers that fetch external data (email, calendar, photos) and normalize it into knowledge-graph facts through the existing fact pipeline. Wired into the workspace `members` and added as a dependency of `mimir-server`.
- **DB-access boundary:** the crate depends on `mimir-core` and `mimir-knowledge` only and never declares a direct `sqlx` dependency; all persistence goes through the `KnowledgeGraph` facade.
- **Feature flags:** `default = ["photos","calendar","gmail"]`; the framework core and mock connector are always built, so `cargo build -p mimir-connectors --no-default-features` still compiles a working framework + mock harness. The flags gate no code yet — backends land in later Phase 3 issues (C1–C7).
- **Stubs:** minimal `Connector` trait, `ConnectorRegistry`, and `MockConnector` placeholders that compile and are object-safe. F6/F7/F13 own the real implementations.
- **Safety:** `#![deny(unsafe_code)]` enforced at the crate root.
- **Tests:** scaffolding smoke test asserting the registry constructs empty and the mock connector implements the trait (passes under all feature combinations).
- **Docs:** added `docs/connectors-framework.md` (technical) and `docs/wiki/connectors.md` (user-facing); updated `README.md`, `docs/workspace.md`, `docs/wiki/what-works-now.md`, and `Mimir-Implementation-Context.md` to register the new crate and reflect Phase 3 as in progress.

## [0.61.3] — 2026-07-01

### Docs — Phase 3 (Connectors) implementation plan

- Added `VISION/09-Roadmap/Phase-3-Plan.md`: the Phase 3 design source of truth. Captures all eight locked architectural decisions (crate structure, extraction reuse/DRY, sync-state + DB-access boundary, orchestration, LLM call queuing, auth/secret storage, multi-backend architecture, tool-vs- connector disambiguation), the version-checked dependency ledger (`oauth2 5.0.0`, `async-imap 0.11.2`, `mail-parser 0.11.4`, `icalendar 0.17.12`, `kamadak-exif 0.6.1`, `keyring 4.1.2`), the 30-issue breakdown across five epics, and the dependency graph. Companion GitHub issues #178–#207 were created and tagged `phase-3`.

## [0.61.2] — 2026-07-01

### Fix — Avoid leaking partially-started workers (PR #177 review)

- (core): `LlmWorkerPool::new` now builds every worker's `LlmClient` up front into a `Vec` and spawns worker tasks only after all clients succeed. A later-iteration `LlmClient::new_direct` failure can no longer leave earlier spawned workers detached with no `LlmWorkerPool` handle to signal shutdown. Added `test_pool_spawns_exactly_configured_workers` regression test and a "Constructor Safety" section to `docs/llm-worker-pool.md`.


## [0.61.1] — 2026-07-01

### Fix — Address PR #177 review feedback

- (client): `normalize_base_url` now rejects non-hierarchical base URLs (e.g. `mailto:`) via `cannot_be_a_base()`, preventing late failures in `url()` / `session_messages()`. Added `try_new_rejects_non_base_url` test.
- (core): Handcrafted HTTP response in the pool in-flight counter test now uses CRLF line endings per HTTP/1.x framing, matching the suggestion from the review.


## [0.61.0] — 2026-06-30

### Optimization & robustness sweep (issues #161–#168)

- **#161** (core): Completed truncated doc comments on `JobError::is_not_registered` and `JobError::is_already_running` (`mimir-core/src/job_queue.rs`).
- **#162** (core): `DailySchedule::parse` now enforces a strict five-character `HH:MM` format with zero-padded fields; non-zero-padded inputs like `"2:30"` are rejected with `JobError::InvalidSchedule` for config-file determinism.
- **#164** (client): SSE stream parser already caps the buffer at 1 MiB (`MAX_SSE_EVENT_SIZE`) and scans delimiters linearly via `memchr` with a resume-from-cursor optimization — verified and documented.
- **#165** (client): Added fallible `MimirClient::try_new(base_url, connect_timeout, timeout) -> Result<Self, ClientError>`; `new` keeps the panicking default for back-compat. Build failures map to `ClientError::Connection`.
- **#166** (core): `LlmClient::new` and `new_direct` are now fallible (`Result<Self, LlmError>`); added `LlmError::ClientBuild`. Daemon startup (`start_server`, `AppState::from_config`) propagates the error instead of panicking; pool workers log and exit on a build failure.
- **#167** (client): DRY'd the repeated `check_response` + `resp.json::<T>()` pattern behind `send_response`/`send_json`/`get_json`/`post_json`/`check_status`; `stop` keeps bespoke 503 handling.
- **#168** (cli): `parse_datetime` interprets offsetless datetimes and date-only inputs in the local timezone (sharing the now-public `DailySchedule::naive_to_utc_local`); explicit RFC3339 offsets are preserved as UTC.

**Breaking (internal API):** `LlmClient::new` / `LlmClient::new_direct` now return `Result`. Internal callers are updated; the OpenAI-compatible HTTP endpoint is unaffected.

## [0.60.7] — 2026-06-29

### Fix — Handle already-fired shutdown trigger in `watch_shutdown`

Addressed PR #176 review feedback (CodeRabbit): `watch_shutdown` could miss a SIGTERM/Ctrl-C fired in the gap between `spawn_os_signal_shutdown` and `shutdown_tx.subscribe()`. `watch::Receiver::changed()` only wakes on *future* updates, so a freshly subscribed receiver whose trigger already fired before subscription would wait indefinitely (until sender drop, which never happens during serving).

**Fix:** Check the current watch value via `borrow_and_update()` before awaiting `changed()`. An already-fired trigger returns immediately; later triggers are still caught by `changed()`.

- `mimir-server/src/lib.rs` — `watch_shutdown` now checks the current value first; added regression test `test_watch_shutdown_handles_already_fired_trigger`.
- `docs/shutdown.md` — documented the subscription-race guard.

## [0.60.6] — 2026-06-29

### Refactor — Single OS-signal listener for graceful shutdown

Addressed PR #176 review feedback (CodeRabbit): `serve_with_bounded_drain` previously built two independent `shutdown_signal` futures — one for axum's `with_graceful_shutdown` and one for the phase-1 serving loop — each registering its own `ctrl_c()`/`SIGTERM` listener. The phase-1 waiter could observe a signal before axum's graceful-shutdown future had registered interest, leaving axum accepting connections until the drain bound kicked in.

**Fix:** Capture Ctrl-C / SIGTERM **once** in a dedicated `spawn_os_signal_shutdown` task that fans the notification into the shared `shutdown_tx` watch channel (the same channel `/stop` writes to). Both axum's graceful-shutdown future and the phase-1 loop now observe that channel via `watch_shutdown`, so they fire in lockstep with no duplicate OS-signal listeners.

- `mimir-server/src/lib.rs` — replaced `shutdown_signal` with `spawn_os_signal_shutdown` and `watch_shutdown`; updated `serve_with_bounded_drain` to use the shared trigger.
- `docs/shutdown.md` — documented the single-listener trigger architecture.


## [0.60.5] — 2026-06-29

### Fix — Daemon no longer self-terminates 30 s after start

The graceful-shutdown drain bound was incorrectly applied to the **entire** server lifetime: `tokio::time::timeout(Duration::from_secs(30), server_fut)` wrapped the whole serving future, so the daemon unconditionally exited 30 s after it began listening — whether or not a shutdown was ever requested. The first `mimir chat`/`mimir ask` after start worked (inside the 30 s window); any command issued later failed with `Mimir is not running.` because the daemon had already exited with status 0 (so `Restart=on-failure` did not relaunch it).

**Root cause:** `mimir-server/src/lib.rs` bounded the server future instead of only the post-signal drain phase.

**Fix:** Extracted `serve_with_bounded_drain`, which splits shutdown into two phases — an **unbounded serve** phase (poll the server concurrently with the shutdown trigger) and a **drain bounded to `GRACEFUL_DRAIN_TIMEOUT` (30 s)** phase (applied only after a trigger fires via Ctrl-C, `SIGTERM`, or `/stop`). A wedged SSE stream can no longer keep the process alive past systemd's `TimeoutStopSec`, and the daemon no longer dies on a fixed timer.

- `mimir-server/src/lib.rs` — `serve_with_bounded_drain`, `GRACEFUL_DRAIN_TIMEOUT`, regression test `test_serve_outlives_drain_timeout`.
- `docs/shutdown.md`, `docs/wiki/daemon-shutdown.md`, `docs/systemd-integration.md` — updated to describe the two-phase shutdown and the unbounded serving lifetime.


## [0.60.4] — 2026-06-29

### Docs — Knowledge Graph documentation audit & gap-fill (#64)

Completed the Knowledge Graph documentation set requested by issue #64 by auditing the existing equivalent docs against the issue's required content and filling stale/missing sections, plus adding the two genuinely missing wiki pages. Existing filenames were kept (DRY; avoids breaking cross-references).

**Technical docs (`docs/`):**
- `knowledge-graph-schema.md` — removed the dropped `entity_dates`/`entity_date_types` tables; documented the events & reminders overlay (`event_types`, `event_statuses`, `auto_complete_policies`, `events`, `pending_event_meta`); corrected the `predicates`→`relationship_types`/`relationship_constraints` rename (migration `031`); fixed lookup-row counts (`relation_types` 3→4, `change_types` 7→9); added `optimization_runs`/`optimization_pass_runs`/`memory_priorities`; completed the migration ordering (023–041); replaced the stale "Entity Dates & Recurrence" and "Future Work" sections.
- `Confidence-Model.md` — added a "Why No Time-Based Decay" rationale and a "Confidence Change Events" table mapping each trigger to its `ChangedBy` actor.
- `inference-engine.md` — added a "How to Add a New Rule" section and replaced the stale "Nightly Optimization" stub list with a reference to the implemented 10-pass pipeline.
- `nightly-optimization.md` — added a per-operation "Transaction Model" section, the `JobPriority` levels table, and a "Trigger & Daemon-Down Handling" note.
- `fact-extraction-pipeline.md` — audited; already current (LLM-orchestrated `remember` tool, sensitivity gate, confirm/reject flow).

**Wiki docs (`docs/wiki/`):**
- `knowledge-graph.md` — reframed as the "second brain" distinct from condensed memory; replaced the dropped entity-dates bullet with events & reminders; added an inference key-concept; replaced the stale "Future Commands (Planned)" with real `mimir kb` examples and a "Relationship to the Wider System" section; corrected the semantic-dedup "future work" note.
- `cli-commands.md` — fixed the stale `mimir memory` section (now KG-backed, not a file) and documented `--refresh`.
- `memory.md` — added "What Appears in Memory vs. What Stays in the Knowledge Graph"; fixed the `mimir kg query`→`mimir kb query` typo.
- `forgetting.md` (new) — soft-delete to a 30-day trash bin, restore, cascade forget, and bulk safeguards (>100 `--yes`, sensitive `--confirm-sensitive`, full-reset `DELETE EVERYTHING` + backup).
- `obsidian-sync.md` (new) — planned export/import design and file format, documented as **not yet implemented and deferred to post-Phase-5**.

## [0.60.3] — 2026-06-29

### Fixed — corroboration docs & nightly recalculation efficiency (#79, PR #174)

- **Documented the pending-confirmation corroboration path.** The confidence-model and "what works now" docs described corroboration only against an existing `Active` fact; `insert_fact_in_tx` also corroborates matching `pending_confirmation` facts. Both docs now state `Active` **or** `pending_confirmation`, matching the implementation.
- **Corrected the wiki corroboration cap statement.** Verified the wiki already states the non-explicit corroboration cap as `0.95` (not `1.1`); wording aligned with the pending-confirmation path for accuracy.
- **Nightly `confidence_recalc` skips already-refreshed rows.** Because each root-aware recalculation cascades to inferred descendants and clears their stale flags in one transaction, later iterations in the stale-fact snapshot could reopen transactions and re-walk subtrees already cleared by an ancestor pass. The loop now re-checks `stale_confidence` cheaply before recalculating, avoiding quadratic work on large stale branches.

## [0.60.2] — 2026-06-27

### Fixed — confidence cascade & nightly recalculation correctness (#79, PR #174)

- **Corroboration at the cap clears `stale_confidence`.** A corroborated fact already at the non-explicit cap (`0.95`) had its confidence unchanged, so the previous delta check skipped the whole update and left the row flagged stale despite new provenance. The update that clears `stale_confidence` now runs whenever corroboration applies, while the `ConfidenceChange` audit entry and the descendant cascade remain gated on an actual confidence delta.
- **Cascade uses a recursion-stack guard, not a global visited set.** `cascade_inner_tx` removed a fact from the visited set when its subtree finished, so a descendant reachable through multiple parents (a diamond graph) is recalculated once per updated parent and ends up with the correct final confidence instead of being skipped after the first parent updates.
- **Nightly `confidence_recalc` updates the stale root fact.** The pass previously only cascaded from each stale fact to its children and never recalculated/cleared the selected row itself, so the same facts could stay stale indefinitely. It now uses a root-aware transactional path (`confidence::recalculate_stale_fact`) that recalculates the stale row (inferred) or just clears its flag (non-inferred), writes a `ConfidenceChange` audit entry only when confidence changes, and then cascades to inferred descendants in the same transaction.

## [0.60.1] — 2026-06-27

### Fixed — corroboration guard consistency (#79)

- The corroboration guard in `insert_fact_in_tx` now treats `System`-sourced new facts as explicit, matching the boost-eligibility check and the documented contract. A `System` fact is no longer able to corroborate (and boost) an overlapping non-explicit fact; explicit facts (`UserEdit`/`System`) only add their source and supersede, never corroborate.

## [0.60.0] — 2026-06-27

### Added — corroboration detection in fact insertion (#79)

- **Corroboration is now resolved inside `insert_fact_in_tx`** for every insert path (extraction pipeline, batch insert, direct `KnowledgeGraph::insert_fact`), within the same transaction as supersession. When a new **non-explicit** fact covers the same claim as an existing `Active` (or pending-confirmation) fact — same `subject_id + relationship_type_id + object`, temporally overlapping `valid_from`/`valid_until` — Mimir adds a source row to the existing fact instead of creating a duplicate, and boosts the existing fact's confidence by `+0.05` per independent corroborating source, capped at `0.95`.
- **Explicit and inferred facts are excluded from the boost.** Explicit (`UserEdit`/`System`) facts stay at `1.0` — corroboration only adds the source for provenance. Inferred fact confidence is structural (recalculated from parents) and is never boosted by a corroborating source.
- **Re-statements are a no-op.** A source with identical provenance (`(source_type, connector_id, raw_reference)`) already recorded against the fact is not an independent corroboration and is skipped, which also avoids the `sources` UNIQUE-index collision.
- **Non-overlapping temporal ranges never corroborate** — they form a timeline of separate facts, matching the existing temporal-facts model.
- **Comprehensive in-transaction confidence cascade.** `cascade_confidence_change` is now transaction-aware (`cascade_confidence_change_in_tx`) and runs unbounded (cycle-guarded by a `visited` set) so a corroborated confidence change propagates to every inferred child for accuracy, with no artificial depth cap. The legacy depth-budget parameter was removed (the only caller, the nightly optimiser, already ran unbounded).
- **Audit + stale flag.** Corroboration writes `SourceAdded` and `ConfidenceChange` audit entries (the latter recording the triggering `source_id`) and clears `stale_confidence` on the existing fact.
- **Removed the pre-insertion `find_existing_fact` stub** in `extract.rs` and the now-unused `ExtractionOutcome.corroborated` / `ProcessResult::Corroborated` plumbing; corroboration is owned by the insert layer.

### Notes

- The `deterministic_dedup_merges_identical_fact_triples` test now sets up its duplicate via a direct SQL insert, because live same-claim non-explicit facts corroborate at insert time and can no longer coexist. The nightly dedup pass remains a safety net for coexisting duplicates (legacy data, direct writes).

## [0.59.1] — 2026-06-25

### Fixed — third pass on PR #173 review feedback

- **`confirm_fact` no longer errors after the confirmation commit.** The overlay-rebuild read of `pending_event_meta` ran after `tx.commit()`, so a `?`-propagated failure would make confirmation look failed to the caller even though the fact was already Active and no longer pending. The read now logs and falls back to the legacy one-time overlay path instead of returning an error (#3).
- **Legacy-fallback test now exercises the future-dated branch.** The `confirm_legacy_pending_fact_falls_back_to_one_time_reminder` test uses a future-dated fixture and asserts the one-time `Reminder` overlay is created; a second test covers the no-`valid_from` (no-overlay) case (#4).

### Notes

- CodeRabbit findings #1 (NULL confidence) and #2 (`event_type_roundtrips` `#[tokio::test]`) are stale re-posts: `facts.confidence` is `NOT NULL` and the test attribute is present at line 117. No code change required.

## [0.59.0] — 2026-06-25

### Fixed — second pass on PR #173 review feedback

- **Sensitive facts preserve event metadata across confirmation.** The extracted recurrence / `event_type` / `auto_complete_policy` / `requires_user_action` are now persisted in a new `pending_event_meta` table at extraction time and used by `confirm_fact` to rebuild the overlay faithfully, instead of synthesising one-time `Reminder` defaults. A confirmed sensitive recurring reminder keeps recurring; a confirmed sensitive task/deadline keeps requiring user action and surfaces as overdue. Legacy pending facts that predate the table fall back to the one-time `Reminder` overlay. This removes the Phase A limitation noted in 0.58.0 (#6).
- **`get_active_recurring` filters past-due rows in SQL.** The advance-pass query now takes the scan `now` and adds `trigger_date < now`, so the twice-daily scan only loads and sorts rows that can actually advance instead of fetching every future recurring event (#7).

### Added

- **Migration 041** — `pending_event_meta` table (fact-keyed event-shape cache for pending sensitive facts, removed on confirm / cascade-deleted on reject).
- **Public API:** `queries::event::{PendingEventMeta, insert_pending_event_meta, get_pending_event_meta, delete_pending_event_meta}`.

### Notes

- CodeRabbit findings #1 (advance filter) and #3 (`event_type_roundtrips` `#[tokio::test]`) were already satisfied by the current code and required no change; finding #2 (NULL confidence) is invalid because `facts.confidence` is `NOT NULL` and the derive and Upcoming queries already share the `confidence >= 0.5` gate.

## [0.58.0] — 2026-06-25

### Fixed — PR #173 review feedback on the events & reminders subsystem

- **Idempotent overlay derivation.** The derive scan now inserts overlays with `INSERT ... ON CONFLICT(fact_id) DO NOTHING` and only counts actual inserts, so a concurrent extraction can no longer trip the `fact_id` unique constraint (#3).
- **Recurring user-action events are no longer auto-advanced.** The advance pass now filters to `Recurring`-policy events with `requires_user_action = false`; recurring deadlines/tasks stay past their trigger date and surface as overdue, matching the documented contract (#4).
- **Sensitive time-bound facts get an overlay on confirmation.** Sensitive facts return `Pending` before the event block; `confirm_fact` now derives a one-time `AutoCompleteOnDate` overlay for future-dated sensitive facts when they are confirmed. Recurrence / `requires_user_action` are not carried across the sensitivity gate in Phase A (documented limitation) (#5).
- **Scan / Upcoming confidence alignment.** The derive query now applies the same `confidence >= 0.5` gate as the Upcoming render, so overlays are only created for facts that will surface (no hidden overlays for low-confidence interaction facts) (#7). Note: `facts.confidence` is `NOT NULL`, so the original "NULL confidence" framing was revised.
- **Calendar-day relative suffix.** `format_upcoming_line` computes the `today` / `in N days` suffix from `date_naive()` differences, so an event early the next calendar day is no longer mislabelled `today` (#8).
- **Docs.** `RecurringYearly` references in `docs/events-reminders.md` updated to `Recurring` to match the renamed policy (#1).

### Added

- **Env overrides for events.** `MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES` (comma-separated `HH:MM`) and `MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS` now flow through `apply_env_overrides_with`, matching the rest of the config (#2).
- **Public API:** `KnowledgeGraph::insert_event_if_absent` and `queries::event::insert_event_if_absent` for idempotent overlay creation.

### Notes

- `event_type` from extraction is intentionally limited to `Task`/`Reminder` in Phase A; the remaining `EventType` variants are seeded for later phases (#6).
## [0.57.0] — 2026-06-25

### Feature — events & reminders subsystem (issue #74)

A smart events/reminders layer for the knowledge graph. Events are modelled as a lifecycle + recurrence overlay on facts: a fact with a future `valid_from` is a one-time event; a fact tagged with recurrence (e.g. a birthday) is a recurring event; a fact flagged `requires_user_action` is a task/deadline. Upcoming events surface automatically in the "Upcoming" memory section.

- **New `events` table** (migration 039) with `event_types`, `event_statuses`, and `auto_complete_policies` lookup tables, keyed on `facts(id)`.
- **`entity_dates` deprecated and removed.** Its recurrence logic (`next_occurrence`) moved to `models::recurrence`; the unused `entity_dates` / `entity_date_types` tables are dropped (migration 040, no data migration required).
- **Deterministic scan job** `events.upcoming_scan` (default 06:00 & 18:00) derives overlays for future-dated facts, auto-completes one-time events past their trigger date, and advances recurring events to their next occurrence. `RequiresUserAction` events stay active and surface as overdue.
- **Extraction bridge.** The `remember` tool schema gains optional `recurrence` and `requires_user_action` fields; the extraction pipeline creates event overlays for qualifying facts (no natural-language date parsing in Rust — the LLM supplies the ISO-8601 `valid_from`).
- **`render_upcoming_section` refactored** to an event-based query, replacing the `entity_dates` and category 900–999 branches.
- **Config:** new `[knowledge.events]` section (`schedule_times`, `horizon_days`).

## [0.56.2] — 2026-06-24

### Bugfix — daemon service reliability

Fixes the three interlocking issues that made the installed `mimir.service` fail to start cleanly and intermittently stop itself.

- **CLI no longer targets the wrong port.** Client commands resolved their base URL from a hardcoded `http://127.0.0.1:8080` (or `MIMIR_BASE_URL`), ignoring the configured `server.bind_addr`. A daemon bound to, e.g. `0.0.0.0:8008` was therefore reported as "not running", the daemon guard prompted to start an already-running service, and the auto-spawned duplicate failed to bind (address in use). The CLI now resolves `MIMIR_BASE_URL` → `server.bind_addr` (wildcard hosts normalised to loopback) → compiled default (`mimir/src/constants.rs`, `mimir-core::config::resolve_base_url`).
- **Cheap `/health` liveness endpoint.** The daemon guard and `mimir stop` probed `/status`, which performs a live LLM round-trip (`fetch_model_context_window`) plus knowledge-graph reads on every call. A slow/unreachable provider made the 500 ms probe time out on a healthy daemon. Added `GET /health` (trivial 200, no LLM/DB work) and pointed the guard + reachability check at it (`mimir/src/daemon_guard.rs`, `mimir-server/src/lib.rs`).
- **SIGTERM shutdown no longer deadlocks.** Only `POST /stop` broadcast the `shutdown_tx` watch channel; the SIGTERM/Ctrl-C path relied on `AppState` being dropped during runtime teardown to release the config file-watcher's `spawn_blocking` thread — a race that, when lost, deadlocked tokio's `BlockingPool::shutdown` until systemd aborted the unit with `SIGABRT` after `TimeoutStopSec` (the "it stops itself" symptom). The server now broadcasts `shutdown_tx` deterministically while the runtime is still alive, and wraps the server future in the documented 30 s `tokio::time::timeout` (`mimir-server/src/lib.rs`).
- **Config file-watcher no longer floods the journal.** Reading the config file generated `Access`/close events that, with only a filename filter, fed a self-reload loop (~1 reload/second even with no real change). The watcher now ignores `Access` events and dedupes by `(mtime, size)` so each genuine content change reloads at most once (`mimir-server/src/lib.rs`).

### Tests

- `mimir-core::config::base_url_tests` — base-URL resolution and config `bind_addr` reading (12 cases).
- `mimir-server::tests::test_health_returns_ok_without_llm` — `/health` does not touch the LLM backend.
- `mimir/tests/e2e.rs::e2e_sigterm_exits_promptly` — the real binary exits promptly on SIGTERM under an isolated environment.

## [0.56.1] — 2026-06-23

### Bugfix

- **Tool-call-start JSON printed to console.** The server emits a `tool_call_start` SSE event (containing `name` and `display_name`) before a tool executes, but the client SSE parser had no arm for that event type, so the raw JSON payload fell through to the default text path and was printed verbatim alongside the formatted result line. Added a `ToolCallStartInfo` type and `StreamItem::ToolCallStart` variant, a matching parser arm, and CLI handling so the event renders as a dim "🔧 DisplayName…" indicator instead of leaking JSON.

## [0.56.0] — 2026-06-23

### Bug & Performance Sweep

Address all open `bug` and `performance` labelled issues. Each was verified before fixing; performance changes include before/after measurements.

#### Bugs

- **#45 — `get_current_time` returns UTC instead of user's time zone.** The tool now returns a structured payload (`local`, `utc`, `offset`) derived from the host's local timezone via `chrono::Local`, so the agent can derive UTC from the offset. The formatting helper is generic over the timezone for deterministic unit testing.
- **#80 — Some config settings don't do anything (temperature).** The LLM client captured `temperature` at startup, so hot-reloaded changes had no effect. Added `LlmBackend::with_temperature_override`; the chat route now applies the live config snapshot temperature per request.
- **#81 — Certain CLI commands don't work.** `mimir chat` accepted no flags and always sent `model`/`personality`/`incognito` as `None`, ignoring `--verbose`. Added `--model`, `--verbose`, `--incognito`, `--personality` flags plus REPL slash-commands (`/model`, `/personality`, `/incognito`, `/verbose`) that toggle at runtime; verbose now reports token usage.
- **#155 — Incognito mode can still write facts via `remember`.** Added a `Tool::is_write_tool` marker (default `false`); `RememberTool` opts in. The chat routes now suppress write-capable tools from the exported tool set and refuse to execute them during incognito turns, so no facts are persisted.

#### Performance

- **#160 — `api-types` leaks `null` fields in KG wire types.** Added `#[serde(skip_serializing_if = "Option::is_none")]` to the sparse `Option` fields of `AuditRow`, `CategoryResponse`, `CategoryDetailResponse`, `TrashRow`, `OptimizationStatusResponse`, `OptimizationRunNowResponse`, and `OptimizationRunSummary`. Sparse payload sizes shrink 42–75% per row (e.g. a sparse `AuditRow` is 64 B vs 123 B; a sparse `CategoryResponse` is 19 B vs 76 B).
- **#163 — `escape_fts5` keeps leading/trailing whitespace in the phrase.** The quoted phrase now uses the trimmed value so padded queries no longer alter FTS5 phrase-matching semantics. `fts5_escape_mixed_inputs` benchmark: 1.41 µs → 1.26 µs (~7% incidental).
- **#164 — SSE stream parser has unbounded buffer growth and O(n²) scan.** The client SSE parser now caps a single event at 1 MiB (emitting a `ClientError` on overflow) and resumes the delimiter scan from the last inspected offset (using `memchr::memmem`), making the cost linear in the event size. Benchmark on the partial-event accumulation path:
  - 1024 chunks: 31.5 ms → 542 µs (~58×)
  - 4096 chunks: 494.9 ms → 1.21 ms (~408×)

### Notes

- `memchr` 2.8 added to `mimir-client`.
- Five pre-existing `mimir-server` KB pending/confirm/reject tests fail on `main` independently of this change (index-out-of-bounds in `insert_pending_fact`); left untouched per scope.

## [0.55.1] — 2026-06-23

### Sensitivity Content Check: Word-Boundary Matching (#142)

Fix a false-positive vector flagged in PR review. The keyword-based content fallback (`is_sensitive_by_content`) previously matched keywords as raw substrings, so benign words containing a sensitive keyword (e.g. "hospitality" contains "hospital", "indebted" contains "debt", "visage" contains "visa") could be confirmed sensitive whenever the LLM also set `is_sensitive=true`.

- **`mimir-knowledge/src/sensitivity.rs`:** `is_sensitive_by_content` now matches each keyword as a whole word using ASCII alphanumeric boundaries via the new private `contains_keyword_word` helper, eliminating embedded-word false positives while still catching genuine single-word uses like "diabetes" or "allergic".
- **Tests:** Added word-boundary regression tests for "hospitality", "indebted", and "visage", plus a trailing-punctuation case and a genuine "hospital" word case.

## [0.55.0] — 2026-06-22

### Rework Sensitivity Detection (#142)

Move sensitivity detection from LLM-only to deterministic Rust validation, eliminating false positives where benign preferences were routed into the pending-confirmation dead end.

- **New module `mimir-knowledge/src/sensitivity.rs`:** Pure, synchronous sensitivity gate with two signals:
  - `is_sensitive_by_category(category_ids)` — checks the fact's catalogue category IDs against the `SENSITIVE_CATEGORIES` constant (health, allergies, financial, romantic, cultural/religious, values/philosophy).
  - `is_sensitive_by_content(object)` — keyword-based fallback for miscategorised facts (e.g. "allergic", "diabetes", "salary", "debt", "divorce", "citizenship").
  - `is_sensitive(llm_flag, category_ids, object)` — combined AND gate: a fact is sensitive only if the LLM flags it **and** Rust agrees. Rust can narrow but never widen.
- **Extraction prompt softened:** "Flag ... Mimir will validate your assessment in Rust."
- **Sensitivity check wired into `process_extracted_fact`** — the single funnel point covering `extract_facts`, `extract_facts_with_context`, and `process_remember_output`.
- **35 unit tests + 7 integration tests** covering all issue acceptance criteria.

## [0.54.5] — 2026-06-22

### Review Fixes (PR #169)

Address CodeRabbit review feedback on the tests-and-benchmarks change set.

- **`mimir-api-types`:** `roundtrip_tests!` sparse-field check now parses the serialised JSON into a `serde_json::Map` and asserts key absence via `contains_key`, instead of the previous substring-based `json.contains` that could match field names inside values.
- **`mimir-client`:** wiremock-backed KB endpoint tests (`kb_query`, `kb_browse`, `kb_profile`, `kb_audit`, `kb_trash`) now assert the expected query-string parameters via `query_param` matchers, catching regressions in query encoding rather than just the route path.
- **`mimir-core`:** `bench_daily_schedule_next_after` uses a fixed `DateTime<Utc>` reference instead of `Utc::now()`, so the benchmark baseline is deterministic and reproducible across runs. Added `daily_schedule_parse_accepts_non_zero_padded_input` to document chrono's padding-agnostic `%H:%M` parsing.
- **`mimir` (binary):** Corrected the misleading comment in `truncate_zero_max_yields_just_ellipsis_or_empty` to state the deterministic "just ellipsis" outcome.

## [0.54.4] — 2026-06-21

### Testing & Benchmarks

Workspace-wide expansion of inline unit tests and pure-helper benchmarks on the `tests-and-benchmarks` branch.

- **`mimir-api-types`:** 12 → 46 unit tests. New `roundtrip_tests!` macro asserts populated + sparse (all-`None`) serde roundtrips and `skip_serializing_if` omission for every KG wire type.
- **`mimir-client`:** ~24 → 64 tests. Adds pure unit tests for the SSE parser primitives (`find_double_newline`, `parse_sse_event`) and wiremock-backed tests for all previously-uncovered `MimirClient` methods.
- **`mimir-core`:** 179 → 211 lib tests. New inline tests for `job_queue`, `tools::{output,permission,error}` pure helpers.
- **`mimir-knowledge`:** ~74 → 110 lib tests. New inline tests for `models::enums`, `retrieval::types`, `inference::rules::transitivity`, `models::{entity_date,memory}` helpers.
- **`mimir-server`:** 50 → 65 lib tests. New `error.rs` tests covering every `ApiError` response helper, including verification that internal error details are masked from clients.
- **`mimir` (binary):** 15 → 29 bin tests. New `kb.rs` tests for `parse_datetime`, `confidence_color`, and `truncate`.
- **Benchmarks:** three new pure-helper suites — `mimir-api-types/wire_types`, `mimir-core/pure_helpers`, `mimir-knowledge/pure_helpers` — covering non-hotpath pathways (FTS5 escaping, confidence scoring, serde roundtrips, schedule arithmetic, tool-output rendering).

### Documentation

- New `docs/unit-tests.md` and `docs/wiki/Testing-and-Benchmarks.md`; `docs/benchmarks.md` updated with the new pure-helper suites.

### Issues

Triaged nine prescriptive follow-ups as GitHub issues #160–#168 (api-types `skip_serializing_if` consistency, doc-comment completion, `DailySchedule::parse` strictness, `escape_fts5` whitespace, SSE buffer DoS, client/LLM construction robustness, client DRY, `parse_datetime` timezone).

## [0.54.3] — 2026-06-21

### Security

- **`mimir-server`:** the sensitive-fact confirmation lifecycle routes (`GET /kb/pending`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject`) are now wrapped in the `require_loopback` middleware, matching the guard already applied to `/kb/optimization/run-now`, `/memory/refresh`, and `/stop`. Only loopback peers can list or mutate pending sensitive facts. No CSRF / `Origin` validation is applied because there is no browser frontend for these routes (the CLI / `mimir-client` is the only client); that hardening belongs to a workspace-wide pass over all mutation routes.

## [0.54.2] — 2026-06-21

### Fixed

- **`mimir-knowledge`:** `extract::reject_fact` now clears `fact_dependencies` rows before the hard-delete. The `fact_dependencies` FK is `ON DELETE RESTRICT` (migration 017), so rejecting a pending sensitive fact that participates in a dependency edge previously hit a foreign-key violation. Mirrors the dependency cleanup already performed by `KnowledgeGraph::delete_stale_pending` and `forget_fact_tx`.
- **`mimir-knowledge`:** `KnowledgeGraph::delete_stale_pending` now re-checks the stale predicate inside each per-fact transaction and only counts committed deletes. A fact confirmed or rejected between the id scan and the delete is skipped rather than incorrectly hard-deleted and given a spurious `Rejected` audit entry.
- **`mimir-knowledge`:** the optimization runner's `pending_confirmation_cleanup` pass now uses the configured `knowledge.pending_cleanup.retention_days` (via a new `OptimizationConfig.pending_cleanup_retention_days` field) instead of a hardcoded 7 days, so the pass and the scheduled `knowledge.pending_cleanup` job share one configured expiry window.
- **docs:** `docs/wiki/facts.md` confirm/reject examples now use the positional `<fact-id>` syntax, matching `cli-commands.md` and `README.md`.

## [0.54.1] — 2026-06-21

### Fixed

- **`mimir-knowledge`:** removed the orphaned, never-called `queries::fact::delete_stale_pending` helper. It duplicated `KnowledgeGraph::delete_stale_pending` with divergent, FK-violating semantics (skipped `fact_dependencies` cleanup and the `Rejected` audit entry). `KnowledgeGraph::delete_stale_pending` is now the single source of truth for stale pending-fact auto-expiry.

## [0.54.0] — 2026-06-21

### Added

- **Pending sensitive-fact confirmation lifecycle (`mimir-server`, `mimir-client`, `mimir`, `mimir-api-types`, `mimir-knowledge`):** the existing internal `confirm_fact`/`reject_fact` APIs are now exposed end-to-end. Sensitive facts (allergies, health, etc.) stored with `pending_confirmation = TRUE` no longer sit in limbo.
  - HTTP routes: `GET /kb/pending`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject` (returns `204 No Content`; optional `reason` body field written to the audit log).
  - CLI commands: `mimir kb pending`, `mimir kb confirm --fact-id N`, `mimir kb reject --fact-id N [--reason "..."]`.
  - API types: `PendingListResponse`, `ConfirmFactResponse`, `RejectFactRequest`, and a public `PendingFactRow`.
  - New `KnowledgeGraph::list_pending_facts()` and `delete_stale_pending()` query methods.

- **Daily pending-fact auto-cleanup job (`mimir-server`):** a new `knowledge.pending_cleanup` background job hard-deletes facts still awaiting confirmation past a configurable retention window. Configurable under `[knowledge.pending_cleanup]` with `retention_days` (default `7`) and `schedule_time` (default `"03:30"`). Implements the 7-day auto-deletion rule described in `VISION/02-Knowledge-Graph/Learning-Modes.md`.

### Changed

- **`reject_fact` now accepts an optional reason (`mimir-knowledge`):** the free function and `KnowledgeGraph` method take `reason: Option<&str>`, threaded through to the audit log. Internal API change (acceptable per the breaking-changes policy).


## [0.53.1] — 2026-06-19

### Fixed

- **Librarian transcript now escapes newlines in message content (`mimir-knowledge/src/extract.rs`):** `\r` and `\n` in `msg.content` are replaced with literal `\r`/`\n` sequences before the `[Role]:` label is applied, preventing a user message containing text like `[Assistant]: …` from forging a labelled line and bypassing the source discipline boundary. Adds a regression test (`prompt_escapes_multiline_content_so_roles_cannot_be_forged`).

### Changed

- **Librarian wiki documentation aligned with implemented novelty check (`docs/wiki/librarian-agent.md`):** the duplicate-handling paragraph now states that facts restating the core-facts block are skipped (not that confidence is "strengthened"), matching the novelty check instruction.

### Tests

- **Assert message-turn shape before indexing (`mimir-knowledge/tests/librarian_agent.rs`):** the `calls[0].len() == 2` assertion now precedes the indexing into `calls[0][0]`/`calls[0][1]` so a shape change fails clearly instead of panicking out-of-bounds.

## [0.53.0] — 2026-06-19

### Changed

- **Librarian extraction prompt redesigned (Issue #139):** the Librarian's `build_extraction_prompt` now composes a KG-focused base (rules, category taxonomy, predicate standards, list splitting, deduplication, output contract — extracted into a shared `build_base_prompt`) with the *same* core-facts block the core agent injects (`Personality::CORE_FACTS_HEADER` + condensed memory, emitted only when non-empty) and the recent conversation rendered as labelled `[User]` / `[Assistant]` messages under `## Recent conversation`. The user's identity is read from the core-facts block by the LLM, exactly as the core agent resolves identity — `UserIdentity` is no longer threaded through the contextual extraction path. A "Source discipline" instruction tells the Librarian to extract facts ONLY from `[User]` messages and never from `[Assistant]` messages (its own prior output), and a "Novelty check" instruction tells it to extract only facts not already present in the core-facts block. The instruction tells the LLM to skip emitting facts that merely restate what is already known (exact duplicates are discarded by Rust regardless of classification), and to use the Correction classification for corrections — avoiding contradictory "strengthen confidence" guidance. The transcript now lives in the system prompt once; the user turn handed to the LLM is a short action instruction, removing the previous duplication.

### Added

- `mimir_core::conversation::{ConversationMessage, MessageRole}` — a labelled transcript message type. `extract_facts_with_context` and `KnowledgeGraph::extract_facts_with_context` now take `&[ConversationMessage]` instead of a `ConversationTurn` + `UserIdentity`, so the amount of conversation context handed to the Librarian can be increased in future without changing the prompt-builder signature. `LibrarianAgent::run` converts the turn into `[User, Assistant]` messages today.
- `Personality::CORE_FACTS_HEADER` is now `pub` so the Librarian reuses the core agent's core-facts label (DRY).

### Removed

- `build_contextual_extraction_prompt` (folded into `build_extraction_prompt`).
- The "Recent related facts about the user" DB snapshot (`get_facts_by_subject`) from the Librarian prompt — novelty checking now relies on the core-facts block.
- `identity: UserIdentity` field from `LibrarianContext` and the `identity` parameter from `extract_facts_with_context`.

### Notes

- This deviates from the original #139 spec (which was outdated): identity is not rendered as a separate prompt line, there is no dedicated recent-facts snapshot section, and the exact `## What I already know about you` / `## Recently learned` / `## Conversation to analyze` headers from the issue are not used. Pronoun-resolution prompting is deferred to the follow-up "Phase 2: Pronoun resolution in fact extraction" issue.

## [0.52.0] — 2026-06-18

### Changed

- **System prompt hardened for the agentic architecture (Issue #138):** the system prompt composed by `Personality::system_prompt` now appends shared operating directives to every preset (built-in and custom). The directives tell the LLM not to invent facts about the user (say so if the answer is not known), to dispatch a retrieval agent via the `retrieve_context` tool when the core facts are insufficient (refining and re-dispatching until answered or confirmed absent), and to call the `remember` tool for explicit assertions, corrections, and meaningful casual mentions — never for chitchat. The injected memory section is relabelled `Core facts about the user` (third person, framed as a condensed subset, not exhaustive). The legacy `Key facts I know about you:` block and its note mentioning `kg_query`/`kg_search`/ `kg_related` are removed; those tools are the retrieval agent's internal tools and are no longer surfaced to the core LLM. The four `built_in_*` presets keep their tone text unchanged — directives are composed once in `system_prompt` for DRY.

### Notes

- **#138 acceptance criteria revised by design:** the "you will receive a synthesized context block" criterion is dropped — Mimir uses LLM-condensed core facts, not a Rust distillation layer (#129 will not ship as written). The "remove the `remember` instruction" criterion is reversed to *encourage* `remember`, matching the #137 inline-LLM-orchestrated learning design and the `test_chat_extracts_facts_after_response` contract. An automatic Librarian fallback (#156) was filed to queue background extraction when `remember` is not called for a configurable number of turns.

## [0.51.0] — 2026-06-18

### Changed

- **Learning is now LLM-orchestrated (Issue #137):** the unconditional background Librarian that re-extracted facts after every non-incognito chat turn has been retired. Fact learning now happens when the conversational LLM calls the `remember` tool inline while composing its reply, and pre-response retrieval stays LLM-driven via the `retrieve_context` tool. The LLM decides *whether* to learn/retrieve; Rust still owns the policy (confidence assignment, overwrite rules, sensitive-fact confirmation) via `process_remember_output`, so the model cannot self-assign confidence or override policy. This reframes issue #137 away from a Rust rule-based intent classifier — NLU is the LLM's job, and orchestration emerges from structured tool selection.
- **`remember` tool description** now summarises the classification semantics (Explicit overwrites, Casual coexists, Correction supersedes) and nudges canonical relationship types, preserving extraction quality without a second LLM call.

### Removed

- `submit_librarian_goal` and its two call sites in the chat route. The `LibrarianAgent`, `LibrarianGoal`/`LibrarianContext`, and `KnowledgeGraph::extract_facts_with_context` remain as a library API for future on-demand/bulk extraction; they are simply no longer auto-invoked.

### Notes

- End-user-visible behaviour: Mimir no longer silently learns from chitchat. It learns when it judges a turn contains worth-remembering information.
- Sensitive facts still require confirmation; the overwrite/coexistence matrix in `VISION/02-Knowledge-Graph/Learning-Modes.md` is unchanged and enforced in Rust.

## [0.50.0] — 2026-06-18

### Changed

- **Predicate resolution is fully data-driven (Issue #136):** the deprecated hardcoded `normalize_predicate` synonym map and the duplicate `normalize_relationship_type` snake_case helper have been removed from `mimir-knowledge/src/extract.rs`. The extraction pipeline now resolves every fact's `relationship_type` through `KnowledgeGraph::ensure_relationship_type`, which consults the `relationship_type_aliases` table (seeded by migrations `036`/`037`) and auto-registers unknown predicates as new canonical types.
- **DRY batch processing:** `process_extracted_facts` and `process_remember_output` now share a single `process_fact_batch` helper, and predicate normalization reuses `normalize_alias` from `mimir-knowledge/src/lib.rs` instead of a local copy. Predicate-resolution errors are tolerated per-fact, so one malformed predicate no longer aborts the whole extraction batch.

### Notes

- End-user behaviour is unchanged: `attended`→`studied_at`, `hobbies`→`hobby`, `works_for`→`works_at`, etc. all resolve via seeded aliases.
- Side effect of routing through `ensure_relationship_type`: an unknown predicate on a fact that is later rejected (e.g. invalid `subject_type`) still registers its canonical type. This is intentional and idempotent.

## [0.49.1] — 2026-06-18

### Fixed

- **Address PR #152 review feedback (Issue #135 ontology seed):**
  - Migration `037` now uses `ON CONFLICT` UPSERTs (not `INSERT OR IGNORE`) for the canonical predicates and their self-aliases, enforcing the canonical `(id, name)` contract on upgrade instead of silently preserving stale mappings.
  - Migration `038` runs inside a transaction with foreign-key enforcement on (`PRAGMA foreign_keys = OFF` removed) and uses `CREATE TABLE/INDEX IF NOT EXISTS` for defensive idempotency.
  - `insert_category_alias` now uses an atomic `INSERT OR IGNORE` + post-insert resolution, eliminating the `SELECT`-then-`INSERT` race that could surface raw `UNIQUE`-constraint errors instead of the documented `Validation` error.
  - `category_aliases_test` re-queries the subtree after inserting the unrelated fact so the exclusion assertion is meaningful.
  - `relationship_ontology_test` self-alias check is now read-only (direct canonical id lookup) instead of mutating the DB via `ensure_relationship_type`.

## [0.49.0] — 2026-06-18

### Added

- **Core relationship ontology (category-first, Issue #135)**: the knowledge graph is now seeded with a category-first ontology. Predicate aliases own verb canonicalization (thin canonical verbs + English synonyms); the Dewey `categories` tree owns grouping, hierarchy, and multi-tag precision.
  - Migration `037` seeds the remaining core predicates (`studied`, `completed_degree`, `educational_status`, `job_title`, `likes`, `dislikes`) with explicit ids 26–31 and self-aliases, so the alias table remains the single source of truth for resolution.
  - Migration `038` adds the `category_aliases` table (`alias` → `category_id`, globally unique) and seeds domain words (`education`, `hobbies`, `residence`, `family`, `identity`, `employment`, `pets`, …) mapping to existing Dewey category nodes. Both migrations are idempotent (`INSERT OR IGNORE`).
  - New `queries::category` helpers: `resolve_category_alias`, `insert_category_alias`, `get_descendant_category_ids` (recursive CTE over `categories.parent_id`), and `get_facts_in_category_subtree` (facts tagged anywhere in a root + descendants). `KnowledgeGraph` exposes thin wrappers for each.
  - Unit tests verify predicate/alias counts, alias resolution, category-alias counts, subtree retrieval, and idempotency across re-init.

### Changed

- **Design shift documented**: grouping/hierarchy is intentionally served by categories, not abstract parent predicates. `relationship_type_hierarchy` is kept but no longer seeded with abstract parents; reworking `kg_query --include_subtree` to expand by category subtree (rather than the predicate DAG) is a tracked follow-up (#134, #136).
- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/what-works-now.md`, new `docs/wiki/categories-and-aliases.md`, and `README.md` to reflect the category-first layering.

## [0.48.1] — 2026-06-18

### Fixed

- **`kg_query` subtree offset contract**: when `include_subtree` is `true` and the requested predicate does not exist (empty result set), the response `offset` is now forced to `0` instead of echoing the caller-supplied `offset`. This closes a gap where the documented "subtree mode disables offset pagination" contract was only honoured on the populated-result path.

## [0.48.0] — 2026-06-17

### Added

- **Relationship-type DAG subtree query (Issue #134)**: facts can now be retrieved for a relationship type and all of its descendants in the `relationship_type_hierarchy` DAG via a SQLite recursive CTE. Querying a broad category (e.g. `education`) returns facts stored under more specific descendant types (`studied_at`, `graduated_from`, …) without the caller needing to know every predicate name.
  - `queries::fact::get_facts_by_relationship_subtree(pool, subject_id, root_type_id, min_confidence, limit)` and the matching `count_facts_by_relationship_subtree` walk the DAG in a single statement, seeding the CTE with the root type so its own facts are included. Filters and ordering match `get_facts_by_subject_filtered` (non-pending, status `NOT IN (5, 6)`, confidence floor, sorted by confidence descending).
  - `KnowledgeGraph::get_facts_by_relationship_subtree(entity_id, root_type_id, limit)` is a convenience wrapper with `min_confidence = 0.0`.
  - `kg_query` gains an `include_subtree` boolean parameter (default `false`). When set with a `predicate`, the predicate (alias-aware) becomes the subtree root; an unknown predicate returns an empty result set, and `include_subtree` without a `predicate` is rejected with `ToolError::InvalidArguments`.

### Changed

- Extracted a shared `enrich_with_sources` helper in `queries/fact.rs` so the exact-match and subtree fact queries share the source-batching logic (DRY).

### Tests

- Added `mimir-knowledge/tests/relationship_subtree_test.rs` covering subtree inclusion of root + descendants, diamond-path deduplication, status/pending/confidence/limit filters, temporal-bound preservation, multi-valued same-type facts, the `KnowledgeGraph` wrapper, and the `kg_query` `include_subtree` parameter (including alias resolution and the predicate-required contract).

### Documentation

- Updated `docs/kg-tools.md`, `docs/knowledge-graph-schema.md`, `docs/wiki/kg-tools.md`, and `docs/wiki/knowledge-graph.md` with the subtree query and `include_subtree` parameter.

## [0.47.0] — 2026-06-16

### Added

- **Relationship type alias resolution (Issue #133)**: `ensure_relationship_type` now resolves incoming names through the `relationship_type_aliases` table before creating a new canonical type. New canonical types automatically register their normalized name as a self-alias, making the alias table the single source of truth for relationship-type lookup.
- Migration `036_seed_relationship_type_aliases.sql` backfills self-aliases for every existing relationship type and seeds the legacy hardcoded synonyms from `extract.rs::normalize_predicate` (e.g., `attended` → `studied_at`) as data-driven aliases.

### Changed

- `mimir-knowledge/src/extract.rs::normalize_predicate` is now deprecated. Fact extraction normalizes predicates to snake_case and resolves aliases through the alias table before list expansion; the hardcoded synonym map remains only as a deprecated fallback.
- `get_relationship_type_id` now resolves aliases through `relationship_type_aliases`, matching `ensure_relationship_type` behavior.

### Tests

- Added `ensure_relationship_type_resolves_alias_to_canonical` and `ensure_relationship_type_creates_new_type_and_self_alias` to `mimir-knowledge/tests/relationship_type_dag_test.rs`.
- Updated existing tests and lookup-sync expectations to account for the seeded relationship type ontology.

### Documentation

- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/knowledge-graph.md`, and `docs/fact-extraction-pipeline.md` to describe alias-aware resolution and the deprecated hardcoded fallback.

## [0.46.1] — 2026-06-16

### Fixed

- **Relationship type alias/canonical collision checks (PR #149 review)**: centralised alias↔canonical collision validation in `mimir-knowledge` and applied it inside the same transaction for every relationship-type write path (`ensure_relationship_type`, `ensure_relationship_type_in_tx`, `insert_relationship_type`, and `insert_relationship_type_alias`). Previously these checks could be bypassed when creating relationship types directly or through the transactional fact-insert path.

### Tests

- Added `relationship_type_dag_test.rs` cases covering:
  - `insert_relationship_type` rejecting a canonical name that shadows an existing alias.
  - `insert_relationship_type` rejecting an alias that shadows an existing canonical name.
  - `insert_facts_batch` (transactional create path) rejecting a relationship type name that shadows an existing alias.

### Documentation

- Updated `docs/knowledge-graph-schema.md` with the collision invariants section.
- Updated `docs/wiki/knowledge-graph.md` with a relationship types overview.

## [0.46.0] — 2026-06-16

### Added

- **Relationship type DAG schema (Issue #132)**: added `relationship_type_hierarchy` and `relationship_type_aliases` tables to `mimir-knowledge`. Relationship types now support a directed acyclic graph (multiple parents allowed) and globally unique English aliases, enabling data-driven predicate discovery instead of hardcoded synonym tables.
- `RelationshipType` and `NewRelationshipType` models in `mimir-knowledge/src/models/relationship_type.rs`.
- New `KnowledgeGraph` API methods: `insert_relationship_type_hierarchy`, `insert_relationship_type_alias`, `resolve_relationship_type_alias`, `get_descendant_relationship_type_ids`, and `get_ancestor_relationship_type_ids`.
- Cycle detection for hierarchy inserts, returning `KnowledgeError::RelationshipTypeCycle`.
- Alias resolution integrated into fact extraction; the legacy hardcoded `normalize_predicate` map remains as a deprecated fallback until the core ontology is seeded.

### Tests

- New `mimir-knowledge/tests/relationship_type_dag_test.rs` covering migrations, DAG traversal, alias resolution, global alias uniqueness, self-loops, indirect cycles, and alias-based predicate normalization.
- Updated `mimir-knowledge/tests/migrations_test.rs` to assert the new tables exist.

### Documentation

- Updated `docs/knowledge-graph-schema.md` with the relationship type DAG design.
- Updated `docs/wiki/what-works-now.md` to list the new DAG + aliases feature.
- Updated `README.md` to mention the relationship ontology layer.

## [0.45.1] — 2026-06-15

### Fixed

- `ConversationTurn` equality and hashing now ignore the `timestamp` field, restoring `AgentRuntime` deduplication of identical `(agent kind, goal)` pairs.
- `AgentRuntime::submit` always removes the pending goal key, even when the agent task panics, preventing permanent leaks in the pending set.
- Chat routes skip Librarian fact extraction when the user message is empty, avoiding wasted LLM calls for empty chat turns.

## [0.45.0] — 2026-06-15

### Added

- **Librarian Agent (Issue #130)**: Replaced the fire-and-forget `spawn_fact_extraction` helper with a reusable background agent. The `Agent` trait and `AgentRuntime` live in `mimir-core`; `LibrarianAgent` lives in `mimir-knowledge`. After each non-incognito chat turn, the route submits a `LibrarianGoal` carrying the full `ConversationTurn`, and the agent extracts facts using the configured user identity, condensed memory, and recent related facts.
- New shared types: `mimir_core::conversation::ConversationTurn` and `mimir_core::identity::UserIdentity`.
- New extraction entrypoint: `mimir_knowledge::KnowledgeGraph::extract_facts_with_context` builds a rich contextual prompt for the `remember` tool.
- New integration tests in `mimir-knowledge/tests/librarian_agent.rs` verify fact extraction from a conversation turn and prompt content.

### Changed

- Chat routes (`/chat` and `/chat/stream`) now submit a Librarian goal instead of spawning `spawn_fact_extraction` directly.
- `test_state_with_config` in `mimir-server` now resolves or creates the configured user entity so background agents can run in server integration tests.

### Documentation

- Added `docs/librarian-agent.md` (technical design) and `docs/wiki/librarian-agent.md` (user-facing overview).
- Updated `docs/fact-extraction-pipeline.md` and `docs/wiki/what-works-now.md` to describe the Librarian Agent.
- Updated `README.md` and `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` to reflect the new agent framework and note future goal-directed research.

## [0.44.0] — 2026-06-14

### Changed
 
 - **Scheduler async mutex**: `BackgroundScheduler` now uses `tokio::sync::Mutex` for pending/running job state and `submit()` is `async`, eliminating clippy `await_holding_lock` warnings and preventing accidental blocking of the async runtime.
 - **Centralized path resolution**: All config/data path construction now routes through `mimir_core::paths`. New helpers added: `skills_dir()`, `history_path()`, `personalities_dir()`. `ToolsConfig::default_path()` and `SkillsPermissionsConfig::default_path()` no longer duplicate `dirs::config_dir()` logic.
 - **Skill permission config placement**: `SkillsPermissionsConfig` moved from the `mimir` binary crate into `mimir_core::skills::permissions_config`, consolidating skill-related persistence in the core library.
 - **DRY HTTP client handling**: `mimir_client` response status handling is centralized in `MimirClient::check_response`, removing duplicated error blocks across every API method.
 - **DRY tool registration**: `mimir-server` startup now registers native tools through a single `register_tool` helper instead of repeating `if let Err(e) = ...` warning blocks.
 - **DRY daemon guard checks**: The `mimir` CLI dispatch loop uses a single `ensure_daemon` helper instead of repeating the same `ensure_daemon_running` error-handling block for every daemon-requiring subcommand.
 - **Shared category API types**: `CategoryResponse` and `CategoryDetailResponse` moved from `mimir-server` into `mimir_api_types` so the server and HTTP client share the same wire types.
 - **Category CLI uses MimirClient**: `mimir kb category` subcommands now use `MimirClient` instead of raw `reqwest` calls, and category methods (`kb_categories`, `kb_category_show`, `kb_category_create`, `kb_category_delete`) were added to the client.
 - **DRY `mimir kb` client construction**: All `mimir kb` handlers now share a local `make_client(base_url)` helper instead of repeating `MimirClient::new(base_url)` at every call site.
 - **DRY kb CLI error handling**: All `mimir kb` handlers use a shared `exit_with_error` helper instead of repeating the same `eprintln!`/`exit(1)` block.
 - **DRY skill/tool command helpers**: `mimir tool/skill` enable/disable/permission handlers use shared `set_*_permission_or_exit` and `persist_*_or_exit` helpers; skill name validation and origin parsing are also shared.
 - **DRY CLI error exits**: Remaining ad-hoc `eprintln!("Error: ...")`/`std::process::exit(1)` blocks in `mimir/src/commands.rs` were routed through the shared `exit_with_error` helper.
 - **Shared CLI error helper exported**: `commands::exit_with_error` is now `pub` so `mimir/src/main.rs` can use it for the `ask` no-query guard, removing another standalone error-exit block.
 - **DRY init/main warnings and errors**: `mimir/src/init.rs` and `mimir/src/main.rs` now use shared `exit_with_error` and a local `warn_on_err` helper, removing duplicated `eprintln!`/`std::process::exit(1)` blocks for daemon startup and systemd activation.
 - **DRY config init**: `Config::init` and `Config::init_at` now share a single `Config::write_default_config` helper instead of duplicating the atomic default-config writing logic.
 - **DRY identity seeding auto-merge**: The accidental-duplicate auto-merge loop in `seed_identity_facts` was extracted into `auto_merge_accidental_duplicates`, flattening nested matches and removing duplicated warning formatting.
 - **DRY best-effort warning helper**: Added a shared `warn_err` helper in `mimir-server` and applied it to tool registration, tools-config loading, alias wiring, and auto-merge, removing repeated `if let Err(e) = ... { tracing::warn!(...) }` blocks.
 - **Single-lock session cache**: `ContextManager::ensure_session_exists` now acquires the `sessions` cache lock once and holds it across the database existence check, removing a redundant second lock acquisition.
 - **DRY HTTP client URL builder**: `MimirClient` now builds endpoint URLs through a private `url()` helper, removing repeated `format!("{}/...", self.base_url)` strings.
 - **DRY environment overrides**: `Config::apply_env_overrides_with` now uses a local `set_from_env!` macro, collapsing dozens of repeated `if let Some(v) = getenv(...) { ... }` blocks into declarative one-liners.
 - **Shared server types**: `CategoryResponse` and `CategoryDetailResponse` are now re-exported through `mimir_server::types` alongside other shared API types.
 - **DRY init error handling**: `mimir init` uses a shared `exit_with_error` helper instead of a one-off `eprintln!`/`exit(1)` block.

### Added
 
 - Unit tests for `SkillsPermissionsConfig` load/save round-trip and invalid TOML handling.
 - Path helper tests for `skills_dir`, `history_path`, and `personalities_dir`.
 - Unit tests for the new `MimirClient` category methods.
 - Unit tests for the `warn_err` best-effort warning helper.
 - Unit test for the `MimirClient::url` helper.
 
 ## [0.43.4] — 2026-06-14

### Fixed

- Replaced all hardcoded `"finish_retrieval"` strings in `RetrievalAgent` with the `FinishRetrievalTool::NAME` constant, removing a maintenance risk if the tool name changes.
- `RetrievalAgent` now executes non-`finish_retrieval` retrieval tool calls concurrently via `futures::future::join_all`, while still assembling tool result messages in the original call order.

## [0.43.3] — 2026-06-14

### Fixed

- `RetrieveContextTool::name()` now returns `Self::NAME` instead of a hardcoded string, keeping the tool name and registry constant in sync.
- `RetrievalAgent::merge_entity_facts` now upgrades an "Unknown" root-entity placeholder when a typed entity with the same name is merged, and skips adding an "Unknown" placeholder when a typed entity already exists. This eliminates duplicate entities across `kg_related` root-entity accumulation and typed results from `kg_query`/`kg_search`.

## [0.43.2] — 2026-06-14

### Fixed

- Added language tags to unlabelled fenced code blocks in `docs/kg-tools.md`, `docs/retrieval-agent.md`, and `docs/wiki/retrieval-agent.md` to satisfy markdownlint MD040.
- Cleaned up the malformed release summary header in `docs/wiki/what-works-now.md` so the version and implemented features are accurate.
- `RetrievalAgent` entity/fact deduplication now preserves full identity: entities are matched by `name` *and* `entity_type`, and facts are compared using all structural and lifecycle fields.
- `RetrieveContextTool` no longer logs the raw retrieval task; it logs only the task length to avoid exposing potentially sensitive user context.
- `retrieve_context` now uses the request-resolved LLM (including per-request model overrides) instead of the startup LLM in both blocking and streaming chat handlers.

## [0.43.1] — 2026-06-12

### Fixed

- RetrievalAgent now emits a tool-result message for `finish_retrieval` even when the LLM erroneously calls it alongside other tools, preventing an unbalanced conversation that could be rejected by the backend.
- `accumulate_kg_query` now parses `valid_from` and `valid_until` from `KgQueryTool` JSON output instead of discarding them as `None`.

## [0.43.0] — 2026-06-12

### Added

- **Agentic context retrieval** (Issue #128). The main LLM can now call `retrieve_context` to launch a dedicated RetrievalAgent. The agent runs an ephemeral, internal LLM session with only retrieval tools (`kg_query`, `kg_related`, `kg_search`, `search_conversation_history`), investigating the knowledge graph and conversation history for up to 25 rounds before returning a structured `RetrievedContext`. This enables multi-step, parallel research for complex questions (e.g. "What should I make for dinner with Mary, Bob, and Tom?").
- New `RetrievedContext`, `RetrievedEntity`, `RetrievedFact`, `RetrievedRelation`, and `ConversationSnippet` types in `mimir-knowledge/src/retrieval/types.rs`.
- `FinishRetrievalTool` — internal termination signal used by the RetrievalAgent to signal completion.
- SSE `event: tool_call_start` in the streaming chat handler, emitted before each tool execution to give users real-time visibility into Mimir's research phase.

### Changed

- Bumped workspace version to 0.43.0.

## [0.42.2] — 2026-06-12

### Fixed

- `init_schema` now only rebuilds the FTS5 index when `messages_fts` is newly created, eliminating unnecessary startup latency and I/O for large conversation histories.
- `seed_identity_facts` inserts identity facts before the alias/auto-merge block, ensuring the canonical entity always has at least as many facts as any qualifying duplicate and preventing `auto_merge_pair` from deleting it.
- Removed dead `escape_fts5` duplicate from `mimir-knowledge/src/queries/entity.rs`; all callers already use `mimir_core::fts5::escape_fts5`.

## [0.42.1] — 2026-06-12

### Fixed

- `seed_identity_facts` now auto-merges bare-name duplicate entities when the preferred name matches an existing entity with ≤2 facts. This resolves the stale-duplicate scenario where a short-name entity was created before the alias was wired up to the canonical entity.
- Added `KnowledgeGraph::count_entity_facts()` helper in `mimir-knowledge` for counting facts referencing an entity as subject or object.

## [0.42.0] — 2026-06-12

### Added

- Added `messages_fts` FTS5 virtual table for full-text search over conversation history (`mimir-core/src/context.rs`).
- Added `ContextManager::search_messages()` for BM25-ranked search with snippet extraction.
- Added `search_conversation_history` built-in tool (`mimir-core/src/tools/builtins/search_conversation_history.rs`).
- Extracted `escape_fts5` to `mimir-core/src/fts5.rs` for shared use across crates.

### Changed

- **Breaking (internal):** Migrated `sessions.id` from `TEXT` (UUID) to `INTEGER PRIMARY KEY AUTOINCREMENT` for faster lookups and smaller storage. All session IDs are now `i64` across the workspace.
- Removed `uuid` dependency from `mimir-core`.
- Incognito session IDs now use negative atomic `i64` counters instead of UUIDs.
- Axum routes now auto-reject non-numeric session IDs with `400 Bad Request` via `Path<i64>`.

### Fixed

- Updated all tests, benchmarks, and integration tests to use integer session IDs.
- Updated API types (`mimir-api-types`), server routes (`mimir-server`), client library (`mimir-client`), CLI (`mimir`), and documentation to reflect integer session IDs.

## [0.41.3] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (additional finding addressed):
  - Added `MIMIR_SCHEDULER_DEBOUNCE_SECONDS` and `MIMIR_SCHEDULER_COOLDOWN_SECONDS` environment variable overrides in `mimir-core/src/config.rs`, following the existing `apply_env_overrides_with` pattern.

## [0.41.2] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (5 findings addressed):
  - Removed unused `jq_for_opt` clone in `mimir-server/src/state.rs` optimisation job closure.
  - Added `DaemonJob::from_job_id()` helper to eliminate duplicated string-to-variant mapping in `mimir-core/src/scheduler.rs`.
  - Log SQL errors in `relationship_type_id` instead of silently swallowing them with `.ok()?`.
  - Clarified memory condensation documentation: separated 2500-character budget from top-N limit (500).
  - Corrected nightly-optimization wiki to state "last minute" instead of "last few minutes" to match the 60-second cooldown default.

## [0.41.1] — 2026-06-11

### Fixed

- `LlmWorkerPool` `in_flight` counter is now incremented and decremented around every job processed by workers. Previously the counter was always zero, causing the scheduler's idle gate to incorrectly allow background jobs while LLM requests were in flight.
- `BackgroundScheduler::submit()` now correctly deduplicates against jobs that are already *running*, not just pending. Prevents back-to-back execution when a submit arrives during an active run.
- `BackgroundScheduler::shutdown()` is now called during `AppState::shutdown()`, wiring the scheduler's private shutdown channel into the daemon's graceful teardown sequence. Prevents stale "Running" DB rows when the runtime drops mid-job.

## [0.41.0] — 2026-06-11

### Added

- Unified `BackgroundScheduler` in `mimir-core` that deduplicates, debounces, and gates all background jobs on user downtime and LLM idle state.
- `DaemonJob` typed enum replaces stringly-typed job IDs for `JobQueue::run_now` and `status`.
- Demand-driven memory condensation: `KnowledgeGraph` emits a `tokio::sync::Notify` on dirty; a listener submits `DaemonJob::MemoryCondensation` to the scheduler.
- Configurable `memory.condensation_top_n` (default 500) replaces hard-coded top-20 hash in condensation pipeline.
- `[scheduler]` config section with `debounce_seconds` (default 5) and `cooldown_seconds` (default 60).
- `LlmWorkerPool` tracks in-flight job count via `in_flight_count()`, exposed through `LlmBackend`.

### Changed

- Replaced fixed 30-second interval loop for auto-condensation with event-driven scheduler.
- `POST /memory/refresh` now uses `force_submit` to bypass scheduler gates.
- Nightly optimization callback now submits condensation through the scheduler instead of direct `run_now`.
- `JobQueue::list_jobs()` added for scheduled-job polling.

### Fixed

- `relationship_type_id` no longer uses `?` on `Result` inside `Option`-returning function (Rust 2024 edition compatibility).

## [0.40.7] — 2026-06-10

### Fixed

- Fact extraction now falls back to parsing the assistant's text content as JSON when the LLM does not emit a structured tool call. This resolves intermittent extraction failures with backends such as Ollama + Gemma that do not support `tool_choice`.
- The daemon guard spawns the background server in its own Unix process group, preventing Ctrl-C in the terminal from killing the daemon.
- `generate_and_install_service_file` now ensures config and data directories exist before writing the systemd unit, preventing NAMESPACE failures when `ReadWritePaths` references missing directories.

## [0.40.6] — 2026-06-10

### Fixed

- Addressed remaining CodeRabbit review feedback for PR #125:
  - Fixed CHANGELOG entry for uninstall.sh redirect typo to use literal characters.
  - Aligned test-only init_at() with production init() by ensuring cache directory exists.
  - Replaced silent unwrap_or((0,)) with explicit match on the fact-count query during auto-merge to avoid treating DB errors as zero facts.
  - Documented the auto-merge threshold (fact_count <= 2) in process_extracted_fact.
  - Optimized category validation in insert_facts_batch to query only referenced category IDs instead of the full table.
  - Tidied SQL formatting in get_facts_by_subject_and_predicate.
  - Documented the alias score adjustment (1.1) in entity search queries.
  - Removed unreachable Windows path checks from the Linux-only resolve_executable_path function.
  - Added defensive mimir substring check in uninstall.sh remove_dir before rm -rf.
  - Removed unused serial_test::serial imports in mimir-core tests.

## [0.40.5] — 2026-06-10

### Fixed

- Fixed fact-loss bug where multiple atemporal facts with the same subject and predicate but different objects (e.g. multiple hobbies) would incorrectly supersede each other, leaving only the last-inserted fact. The temporal overlap logic in `insert_fact_in_tx` now respects a `MULTI_VALUED_PREDICATES` allow-list (`hobby`, `likes`, `has_pets`, `has_sibling`, etc.) so that independent values for these predicates coexist instead of overwriting one another.

## [0.40.4] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (additional findings addressed):
  - Fixed typo in `scripts/uninstall.sh` where `error()` redirected with `&&2` instead of `>&2`.
  - Fixed `insert_facts_batch` atomicity by calling `ensure_relationship_type_in_tx` inside the batch transaction instead of autocommitting via `ensure_relationship_type`.
  - Moved `preferred_name` alias registration and auto-merge side effects in `process_extracted_fact` to after the dedup/corroboration check, preventing irreversible mutations on duplicate facts.
  - Aligned `generate_service_file` implementation with its docs and test by removing the unused `cache_dir` parameter and updating callers.

## [0.40.3] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (8 findings addressed):
  - Strengthened `normalize_predicate` to handle `name` → `has_name`, `nickname` → `preferred_name`, `favorite_food`/`color`/`colour` variants, and trimmed leading/trailing whitespace.
  - Expanded `LIST_PREDICATES` to include `has_pets`, `has_child`, `has_parent`, `has_sibling`, and `has_partner`.
  - Removed extra whitespace from the `remember` tool description.
  - `remember` tool output now includes actual error messages instead of just counts.
  - Replaced flaky `tokio::time::sleep(200ms)` in chat integration test with a deterministic polling loop and timeout.
  - `spawn_fact_extraction` now skips empty/whitespace-only messages.
  - Renamed `user_message_clone` to `user_message` in `chat_stream_handler` to clarify ownership.
  - Optimized `seed_identity_facts`: replaced full 1,000-fact scan with targeted predicate-specific queries; both identity inserts are now performed atomically via `KnowledgeGraph::insert_facts_batch`.

### Changed

- Added `relationship_type_id`, `get_facts_by_subject_and_predicate`, and `insert_facts_batch` to `KnowledgeGraph` API.

## [0.40.2] — 2026-06-10

### Fixed

- **Chat fact extraction wired up**: The fact-extraction pipeline (`mimir-knowledge/src/extract.rs`) was fully implemented but never triggered from chat. Both `/chat` and `/chat/stream` endpoints now spawn a background task after persisting the assistant response to extract facts from the user message. This fixes the long-standing issue where Mimir could query the knowledge graph but never write to it from conversation.
- **DRY refactor**: Extracted the duplicated extraction-spawning logic into `spawn_fact_extraction` in `mimir-server/src/routes/chat.rs`.

- **`remember` tool**: Registered `RememberTool` in the tool registry so the LLM can proactively write facts during conversation. The tool accepts structured `RememberOutput` and processes each fact through the same validation, dedup, confidence-assignment, and insertion pipeline used by background extraction.
- **System prompt updated**: The injected memory note now tells the LLM to use the `remember` tool whenever the user shares something worth saving.
- **Extraction prompt enriched**: Added detailed predicate standards (e.g., `studied_at` not `attended`, `hobby` not `hobbies`), explicit list-splitting instructions, and deduplication guidance to the fact extraction system prompt.
- **Predicate normalisation**: Rust-side `normalize_predicate` maps common LLM synonyms to canonical names (e.g., `attended` → `studied_at`, `hobbies` → `hobby`).
- **Comma-separated list splitting**: `split_list_objects` expands single facts with comma-separated values into multiple independent facts for allow-listed predicates (e.g., `hobby: "A, B, C"` → three separate `hobby` facts).

### Changed

- **Documentation**: Updated `docs/fact-extraction-pipeline.md`, `docs/chat-server.md`, and `docs/wiki/fact-extraction.md` to reflect that extraction is now live in the daemon.

## [0.40.1] — 2026-06-10

### Fixed

- **Personality prompts**: Removed references to a non-existent `memory` tool from `transparent`, `concise`, and `warm` presets. The LLM was instructed to use a tool that was not registered, causing `ToolError::NotFound("memory")` during conversations.
- **Identity seeding**: When the server starts, it now inserts `has_name` and `preferred_name` facts into the knowledge graph for the user entity (if not already present). This ensures Mimir can learn the user's identity through the existing memory condensation pipeline instead of relying on prompt injection.

## [0.40.0] — 2026-06-10

### Added

- **Issue #63**: Comprehensive testing suite for `mimir-knowledge`.
  - Inline unit tests for confidence model, clock, entity/fact models, and forget logic.
  - Temporal point-in-time DB integration test (`tests/temporal_point_in_time.rs`).
  - Criterion benchmark suite (`mimir-knowledge/benches/kg_benchmarks.rs`) with 10k-fact dataset covering entity resolution, FTS5, graph traversal, inference chain, and memory condensation.
  - `Clock::today()` and `MockClock::advance(Duration)` for deterministic temporal testing.

### Changed

- `MockClock::advance_seconds(i64)` replaced with `advance(&self, duration: Duration)`.


## [0.39.0] — 2026-06-10

### Added

- **Issue #61**: Full `mimir kb` CLI command suite (Phase A).
  - New commands: `kb query`, `kb show`, `kb edit`, `kb browse`, `kb profile`.
  - Existing commands (`kb audit`, `kb forget`, `kb restore`, `kb trash`) rewritten to go through the daemon via HTTP instead of opening SQLite directly.
  - All commands support `--json` for scripting output.
  - Human-readable output uses `tabled` for tables and `colored` for confidence color-coding (green >0.9, yellow 0.7–0.9, red <0.7).
  - Server routes added under `/kb/`: `query`, `facts/:id`, `facts/forget`, `browse`, `profile`, `audit`, `trash`, `trash/restore`.
  - Shared API types added to `mimir-api-types`: `FactQueryParams`, `FactDetailResponse`, `FactEditRequest`, `BrowseRequest`, `ProfileRequest`, `AuditQueryRequest`, `ForgetRequest`, `RestoreRequest`, `TrashListResponse`, and supporting row types.
  - New `update_fact` method in `mimir-knowledge` for structured field editing with transactional audit logging.
  - Server integration tests for all new routes.

### Changed

- CORS configuration now allows `PATCH` and `DELETE` methods.

## [0.38.0] — 2026-06-09

### Changed

- **Issue #112**: Switched chat context injection wording from `## Persistent Memory Context` to `Key facts I know about you:`.
  - Signals to the LLM that the injected memory is a curated subset, not an exhaustive record.
  - LLM should continue to use KG tools (`kg_query`, `kg_search`) for deeper or exhaustive queries.
- Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs` to use the new wording.
- Updated unit and integration tests in `mimir-core` to assert the new prompt text.

### Added

- Added server integration tests in `mimir-server/src/lib.rs`:
  - `test_chat_injects_kg_memory_into_system_prompt`: verifies blocking `/chat` injects KG condensed memory into the system prompt.
  - `test_chat_stream_injects_kg_memory_into_system_prompt`: verifies SSE `/chat/stream` injects KG condensed memory into the system prompt.


## [0.37.0] — 2026-06-08

### Removed

- **Issue #111**: Deleted the legacy `memory.md` file-backed memory system entirely.
  - Removed `mimir-core/src/memory/` directory (`MemoryManager`, `MemoryLoader`, `MemorySnapshot`).
  - Removed `MemoryTool` from `mimir-core/src/tools/builtins/`.
  - Removed `memory_manager` benchmark from `mimir-core`.
  - Cleaned stale `# path = "${CONFIG_DIR}/memory.md"` example comments from config TOML strings.

### Changed

- Memory is now exclusively knowledge-graph-backed via `mimir-knowledge`.
- `mimir-core` no longer exports a `memory` module; all memory access flows through `mimir-knowledge::KnowledgeGraph`.


## [0.36.0] — 2026-06-08

### Removed

- **Issue #110**: Removed all remaining file-based memory.md scaffolding.
  - memory.path and MIMIR_MEMORY_PATH env override removed from MemoryConfig.
  - MemoryTool unregistered from daemon and CLI tool list.
  - MemoryLoader::init() no longer called during mimir init.
  - AppState no longer carries memory_path or syncs memory.md on shutdown.
  - StatusResponse no longer includes memory_path.
  - mimir-core/src/paths.rs no longer exports memory_path().

### Changed

- mimir memory CLI and /memory server route now exclusively serve knowledge-graph-backed condensed memory.
- mimir status and chat REPL /status display no longer show the deprecated memory.md path.

### Added

- CLI parsing test for mimir memory --refresh flag.

### Documentation

- Updated docs/memory-system.md, docs/cli.md, docs/chat-server.md, docs/shutdown.md, docs/wiki/memory.md, docs/wiki/what-works-now.md, docs/wiki/cli-commands.md, docs/wiki/configuration.md, and docs/wiki/tools.md to remove memory.md references and describe the KG-backed system.

## [0.35.3] - 2026-06-08

### Fixed
- Fixed `sqlx::migrate!` not recognising `-- no-transaction` in migrations 031, 032, and 033 because the directive was preceded by comment headers. This caused those migrations to run inside transactions, which in turn caused `PRAGMA foreign_keys = OFF` to be ignored. Migration 033's `DROP TABLE relationship_types` then triggered an `ON DELETE CASCADE` that silently emptied `relationship_constraints`, breaking `test_predicate_validation`.


## [0.35.2] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback (CodeRabbit AI):
  - Removed duplicate 0.35.1 section from CHANGELOG.
  - Fixed oversize LLM output handling in memory condensation to use deterministic fallback instead of truncation, preventing underflow at `char_limit == 0`.
  - Recurring event output now uses the computed next occurrence date instead of the stored historical date.
  - Search failures during user entity resolution are now handled separately from "not found", preventing duplicate entity creation on transient errors.
  - Memory condensation job failures are now propagated to the job queue result instead of being silently swallowed.
  - Auto-trigger condensation loop is now skipped when no user entity is configured, preventing perpetual 30-second re-triggers.
  - `mimir init` now falls back to system identity when blank/whitespace input is provided.
  - `mimir memory --refresh` now surfaces server-side errors in the CLI output and exits with a non-zero status on failure.
  - Added client tests for `memory_refresh()` success and error paths.
  - Added server route tests for `/memory/refresh` non-loopback rejection, not-registered, and already-running cases.

## [0.35.1] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback:
  - Status endpoint now reads live condensed memory and upcoming section from the knowledge graph instead of the deprecated `memory.md` file.
  - `condensation_dirty` flag now automatically triggers the memory condensation job via a background watcher in the daemon.
  - Removed unused `whoami` dependency from `mimir-core`.
  - Removed dead `condensation_queued` field from `AppState`.
  - Centralised `recurrence_type_id` to `RecurrenceType` mapping via `TryFrom<i16>` in the enums module.
  - Chat system prompt builder now logs warnings when knowledge graph memory queries fail.
  - DRYed the SQL query in `build_memory_schema_with_opts` by constructing it once with a conditional predicate.
  - Fixed budget truncation loop so facts in `exclude_from_budget` buckets are still collected after the character budget is exhausted.

## [0.35.0] — 2026-06-07

### Added
- **Live Memory System (Issue #109)** — Replaced static `memory.md` with an event-driven, knowledge-graph-backed memory block.
  - Stable facts are condensed by the LLM and cached in `system_state.condensed_memory`.
  - Upcoming events (entity dates + temporal facts) are rendered fresh on every request.
  - Regeneration triggers: fact mutations, explicit `mimir memory --refresh`, and nightly optimization completion.
  - Pure formatting LLM prompt with deterministic fallback on failure or oversized output.
  - Sensitive facts are excluded from the LLM condensation pipeline.
- **Identity configuration** — `mimir init` now prompts for full name and preferred name, stored in `[identity]` config section.
- **User entity auto-resolution** — Daemon resolves the user entity from config at startup, creating it in the KG if missing.

### Changed
- `/memory` HTTP route now returns the live condensed memory block instead of `memory.md`.
- Chat system prompt now injects the live memory block from the knowledge graph.
- `build_memory_schema` supports `exclude_buckets` and `exclude_sensitive` options.
- `OptimizationRunner` now supports an `on_complete` callback for post-optimization hooks.

### Deprecated
- `memory.md` file-based memory is deprecated. `MemoryTool` writes are now logged as warnings.

## [0.33.2] - 2026-06-05

## [0.34.2] - 2026-06-07

### Fixed

- **Addressed PR #113 review feedback** (CodeRabbit AI review round 2):
  - Added serde default for `memory_priority_id` in `Fact` model to preserve legacy trash payload deserialization.
  - Replaced magic priority ID fallback (`3`) with semantic SQL lookup against `memory_priorities` table.
  - Fixed fire-and-forget centrality cache updates by making `bump_centrality` and `drop_centrality` async.
  - Eliminated TOCTOU race in `build_memory_schema` cache population with a read-then-populate pattern.
  - Replaced hardcoded category ID lists in `determine_bucket` with named constants.
  - Fixed potential UTF-8 panic in `truncate_fact` with char-aware truncation.
  - Reformatted SQL strings across `trash.rs` and `inference_tests.rs` for readability.
  - Updated documentation version references and corrected incomplete sentences.

## [0.34.1] - 2026-06-06

### Fixed

- **Review fixes for PR #108**: addressed 3 critical review findings in fact ranking engine.
  - Wired up `memory_priority_id` from `relationship_types.default_memory_priority_id` during fact insertion (`queries/fact.rs`, `extract.rs`, `models/fact.rs`).
  - Moved `drop_centrality` cache decrements to occur **after** `forget_fact` database transaction succeeds (`lib.rs`), preventing permanent cache drift on DB errors.
  - Fixed `truncate_fact` budget edge case (`queries/memory.rs`) so that when remaining budget is smaller than `subject + relationship + 3` overhead, `object_display` is correctly truncated to `…` instead of silently exceeding the budget.


### Fixed

- **Review fixes for PR #107**: addressed 10 CodeRabbit review findings across knowledge graph, server, and CLI.
  - `extract.rs` prompt now includes sub-categories with indentation so the LLM can pick specific IDs.
  - `lib.rs` fact insertion now validates category IDs before `INSERT OR IGNORE`, failing loudly on non-existent categories.
  - `queries/category.rs` replaced magic `NOT IN (5, 6)` with bound `FactStatus::Superseded` / `Forgotten` parameters.
  - `kg_expand_catalogue.rs` now queries real `fact_count` for each child category instead of hard-coding `0`.
  - `integration_tests.rs` merge assertion tightened with `object_id` filter to avoid false positives.
  - `error.rs` no longer leaks raw internal KG error strings in `500` HTTP responses.
  - `lib.rs` (server) tool-registry tests now assert `expand_catalogue` and `get_facts_in_catalogue` are exported.
  - `chat.rs` only fetches the catalogue DB when a new session or incognito turn starts, avoiding hot-path latency.
  - `cli.rs` `category add` now exposes `--memory-weight` to match the server API.
  - `kb.rs` JSON decode failures are no longer swallowed with `unwrap_or_default()`; they now surface as fatal CLI errors.

## [0.33.1] - 2026-06-05

### Fixed

- **P2**: `get_facts_matching_all_categories` now deduplicates input category IDs before querying, preventing empty results when duplicate IDs are passed.
- **P3**: Removed unused `client` variable in `mimir/src/kb.rs` (`handle_kb_category`).
- **P3**: Simplified redundant closures in `mimir-server/src/routes/kb_categories.rs` (5 instances of `.map_err(|e| error::knowledge_error(e))?` → `.map_err(error::knowledge_error)?`).

## [0.32.2] - 2026-06-05

### Fixed

- **Review fixes for PR #92**: addressed 14 CodeRabbit review findings across job queue, optimization pipeline, documentation, and daemon routes.

## [0.32.1] - 2026-06-04

### Fixed

- **P1**: `optimization_pass_runs` now linked to parent `optimization_runs` via foreign key `run_id`. `OptimizationRunner` inserts a parent row at pipeline start and updates it on completion or failure. Failed passes are recorded with error text instead of being silently omitted.
- **P1**: `DailySchedule::next_after` now converts the stored naive local time to UTC using `chrono::Local`, fixing scheduling for non-UTC timezones.
- **P1**: `chat_stream_handler` now calls `state.record_user_activity()`, ensuring SSE stream interactions update `last_user_activity` and prevent premature job yielding.
- **P2**: `JobQueue::run_now` now rejects concurrent executions of the same job by checking for an existing `Running` row in `job_runs`.
- **P2**: `semantic_dedup` candidate query now includes `ORDER BY a.id, b.id` for deterministic candidate selection.
- **P2**: `semantic_dedup` now uses a structured LLM tool schema (`evaluate_dedup_candidates`) instead of relying on raw JSON parsing from a plain-text prompt.


## [0.32.0] - 2026-06-04

### Added

- JobQueue and nightly optimization pipeline (issue #58):
  - New `mimir-core::job_queue` with durable job definitions, runs, scheduling, and manual triggers.
  - `JobQueue` persisted in `jobs.db` with `Job`, `JobPriority`, `JobStatus`, `JobRunStatus`, `DailySchedule`, `JobContext`, and `JobRunSummary` public types.
  - Config support for `[knowledge.optimization]` defaults: `cpu_cores = 1`, `nice_level = 10`, `timeout_minutes = 120`, `schedule_time = "02:00"`.
  - Daemon tracks user activity in `AppState`; chat routes record interaction time.
  - System jobs yield between pass boundaries when user activity is inside the 5-minute idle window.
  - Daemon routes: `GET /kb/optimization/status` and `POST /kb/optimization/run-now` (loopback-only for run-now).
  - CLI commands: `mimir kb optimization --status` and `mimir kb optimization --run-now`.
  - Refactored `mimir-knowledge/src/optimization` into pass modules with 10 nightly passes (7 core optimization passes plus 3 cleanup steps):
    - Pass 1: deterministic dedup (exact triple merge).
    - Pass 1b: semantic dedup via LLM structured JSON; auto-merge >= 0.9 confidence, queue uncertain pairs.
    - Pass 2: contradiction resolution.
    - Pass 3: inference chain re-evaluation.
    - Pass 4: confidence recalculation.
    - Pass 5: dormant cleanup (old disputed non-user facts).
    - Pass 6: pattern consolidation stub.
    - Pass 7: compaction (FTS rebuild, ANALYZE, VACUUM).
    - Plus: pending confirmation cleanup (7-day TTL) and trash cleanup.
  - Pre-pass backup with `VACUUM INTO` to `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db` with counter suffix for collisions.
  - Per-pass run recording in `optimization_pass_runs` table.
  - Integration tests for daemon routes and CLI client methods.

### Changed

- `run_nightly_optimization` compatibility wrapper now delegates to `OptimizationRunner::run_all`.
- `cascade_inner` in `confidence.rs` future is now `Send`-safe.

## [0.31.1] - 2026-06-04

### Fixed

- **P1**: `restore_all` now maps both child and parent IDs through `id_map` when rebuilding `fact_dependencies`, preventing FK violations on restored facts.
- **P1**: `restore_fact` now marks the trash row as restored, preventing duplicate restores and stale trash listings.
- **P1**: `hard_delete_all_facts` correctly reports the number of forgotten facts via `rows_affected()` instead of querying the now-empty table.
- **P1**: `create_backup` escapes single quotes in the backup path before interpolating into `VACUUM INTO`, preventing SQL injection/breakage from `XDG_DATA_HOME` paths containing apostrophes.
- **P2**: Restoration audit log now references the newly generated fact ID instead of the original deleted ID.

## [0.31.0] - 2026-06-04

### Added

- Phase 2: Forgetting system -- trash, cascade forget, restore, bulk operations (#57)
  - Bulk forget by predicate, entity, source, time range, and full reset.
  - Trash bin with 30-day expiry, restoration, and automatic nightly cleanup.
  - Cascade forget for inferred facts: orphan removal and confidence recalculation.
  - Bulk safeguards: >100 facts requires --yes, sensitive predicates require --confirm-sensitive, full reset requires typing DELETE EVERYTHING.
  - Full reset creates a timestamped SQLite backup via VACUUM INTO.
  - New CLI commands: mimir kb forget, mimir kb restore, mimir kb trash.
  - Extended TrashPayload with dependency chains so restored facts rebuild parent links.
  - Sensitive predicate flag (sensitive BOOLEAN) on predicates table with seeded defaults for medical/financial terms.


## [0.30.1] - 2026-06-04

### Fixed

- **P1**: `kg_query` and `kg_related` no longer mutate the database via `ensure_predicate` during read-only tool calls. Both now use the new read-only `get_predicate_id` lookup; missing predicates return empty results instead of silently inserting rows.
- **P2**: `AppState` knowledge graph and context database fallbacks now propagate `PathsError` instead of using a broken tilde (`~`) literal path.
- **P3**: `kg_search` now returns an explicit invalid-arguments error when an unrecognized `entity_type` is supplied, rather than silently ignoring the filter.

## [0.30.0] - 2026-06-04

### Added

- Phase 2: Knowledge Graph LLM tools — `kg_query`, `kg_related`, `kg_search` (#56)
  - Database migration `028_add_performance_indexes.sql` for tool query performance.
  - Query layer: `search_entities`, `traverse_graph`, `get_facts_by_subject_filtered`, `get_entity_names`.
  - Tool implementations in `mimir-knowledge/src/tools/` implementing `mimir_core::Tool`.
  - Server integration: `AppState` initialises `KnowledgeGraph` and registers all three tools.
  - Input sanitisation, FTS5 injection defence, and SQL-level exclusion of pending/superseded/forgotten facts.
  - Comprehensive unit and integration tests.


## [0.29.2] - 2026-06-03

### Fixed

- `mimir-knowledge/src/optimization/mod.rs`: `cleanup_stale_pending_confirmations` now deletes `fact_dependencies` rows before deleting the fact and wraps each deletion in a transaction, avoiding `ON DELETE RESTRICT` violations and ensuring atomic DB/cache state.

## [0.29.1] - 2026-06-03

### Fixed

- `mimir-knowledge/src/extract.rs`:
  - `confirm_fact` now cascades inferred facts instead of discarding them (P1).
  - `find_existing_fact` dedup query now matches pending-confirmation facts, preventing duplicate sensitive extractions (P1).
  - `handle_correction` retrospective loop is now atomic: all overlapping facts are marked `Corrected` and soft-deleted in a single transaction before child evaluation (P2).
- `mimir-knowledge/tests/extraction_test.rs`: corrected misleading comment in `test_casual_extraction` (P3).

## [0.29.0] - 2026-06-03

### Added

- Fact extraction pipeline (issue #55):
  - `mimir-knowledge/src/extract.rs`: full LLM → Rust validation → entity resolution → confidence assignment → sensitive confirmation → fact insertion pipeline.
  - LLM tool `remember`: structured schema for extracting subject-predicate-object triples with classification (Explicit / Casual / Correction), temporal bounds, and sensitivity flags.
  - Entity resolution: names matched via exact → alias → FTS5 fuzzy; new entities auto-created with LLM-provided type.
  - Confidence assignment: classification maps to `SourceType` → `confidence::initial()`; LLM hints are ignored.
  - Correction handling:
    - Temporal: `correction_scope` as ISO-8601 datetime closes the sole open-ended predecessor.
    - Retrospective: `correction_scope = "always"` marks overlapping facts as `Corrected`, moves them to trash, and inserts the new fact.
  - Sensitive fact confirmation flow:
    - Sensitive facts inserted as `Disputed` with `pending_confirmation = TRUE`.
    - In-memory `HashSet<i32>` cache rebuilt from DB on startup.
    - `confirm_fact`: flips to `Active`, confidence `1.0`, triggers inference.
    - `reject_fact`: hard-deletes with `Rejected` audit entry.
  - Corroboration stub for issue #79: duplicate facts returned in `ExtractionOutcome::corroborated` without insertion.
  - 11 integration tests covering explicit, casual, entity resolution, temporal/retrospective correction, sensitive confirmation/rejection, multiple facts, empty extraction, and invalid LLM output.

### Changed

- `facts` table: added `pending_confirmation BOOLEAN NOT NULL DEFAULT FALSE` (migration 026).
- `change_types` table: added `rejected` (migration 027).
- `Fact` model: added `pending_confirmation` field.
- `ChangeType` enum: added `Rejected = 8`.
- `ranges_overlap` in `queries/fact.rs`: made `pub` for reuse in extraction pipeline.


## [0.28.1] - 2026-06-02

### Fixed

- Review feedback on inference engine (issue #54):
  - `CHANGELOG.md`: reordered 0.28.0 section to top with markdownlint blank lines.
  - `docs/inference-engine.md`: explicit facts are detected by `!inferred` rather than `confidence == 1.0`.
  - `mimir-knowledge/src/inference/mod.rs`: streaming evaluation for `evaluate_batch` (pending — rule loop still materialises; moved to follow-up).
  - `contradiction.rs`: explicitness uses `!inferred`; status updates wrapped in atomic transactions via `set_status_tx`.
  - `threshold.rs`: DB errors propagated instead of `unwrap_or(0)`; stale preferences deleted when source fact missing; duplicate `StatusChange` audit entries deduplicated within 24h.
  - `transitivity.rs`: trigger queries include `FactStatus::Inferred`; inferred facts use temporal intersection of parent windows.
  - `lib.rs`: `ensure_predicate` insert is atomic with `ON CONFLICT`.
  - `NewFact`: removed `Default` impl; added `NewFact::new(subject_id, predicate)` constructor.
  - `optimization/mod.rs`: confidence cascade uses unlimited depth (`None`); operational errors propagated instead of swallowed.
  - Tests: predicate name roundtrip restored; unknown predicate test uses absent ID; contradiction relation type asserted; cycle-safety contract replaces brittle exact count.

## [0.28.0] - 2026-06-02

### Added

- Inference engine core with `InferenceRule` trait, `RuleEngine`, and `CascadeContext` for cycle-safe unbounded cascades.
- Transitivity rule: `visited`/`is_in` + `is_in` chain → inferred transitive facts with depth-tracked confidence.
- Contradiction rule: real-time `Disputed` status + bidirectional `Contradicts` edges; nightly batch auto-resolves explicit > inferred disputes.
- Threshold rule: 3+ `rejected_action` facts → `General` preference upsert; nightly re-count warns if threshold drops.
- `PredicateRegistry` with `ensure_predicate` and `predicate_name` for unlimited extensible predicates backed by the DB.
- Migrations 024 (Contradicts relation type) and 025 (rejected_action predicate).
- Nightly optimization orchestrator (`run_nightly_optimization`) wiring contradiction resolution, confidence propagation, and inference re-evaluation.
- Integration tests for transitivity, contradiction, threshold, cascade, and cycle safety.

### Changed

- Removed compile-time `Predicate` enum; `NewFact.predicate` is now a `String` resolved at runtime.
- `Fact::predicate()` removed; callers use `kg.predicate_name(fact.predicate_id)`.
- `KnowledgeGraph::insert_fact` automatically runs inference rules and cascades inferred facts.
- `NewFact` extended with `inferred`, `inference_depth`, `confidence`, and `parent_fact_ids` fields.

### Documentation

- Added `docs/inference-engine.md` with architecture, rule descriptions, confidence formulas, and cascade behavior.
- Added `docs/wiki/inference-rules.md` with user-facing examples and best practices.

## 0.27.1 (2026-06-02)

> Next-day hotfix release for 0.27.0.

### Fixed

- Atomic upsert: delete and insert now happen in a single transaction, preventing data loss on crash between commit and insert.
- Contextual lookup now correctly falls back to the default (zero-context) preference when no contexts match, instead of ranking by confidence.
- `preference_sources` now binds `extracted_at` explicitly for deterministic timestamps.
- `preference_audit_log` stores `NULL` for the `reason` column on creation events instead of an empty string.
- `get_preference` eliminates N+1 queries by fetching all contexts in a single query.
- Uniqueness checks in `insert_preference` and `upsert_preference` no longer clone the full context `HashSet`.
- Confidence validation now happens before acquiring a database write lock.
- Migration 023 now seeds `predicate_constraints` for `HasPreference` so `validate_predicate` does not fail.

## 0.27.2 (2026-06-02)

### Fixed

- Review feedback on preference system (issue #53):
  - `source_fact_id` is now nullable in `preferences` table and Rust types (`Option<i32>`).
  - Explicit preferences (`overridden_by_user = true`) now require `confidence = 1.0` at validation time.
  - `UpsertAction::Overwritten` now updates the existing preference row in-place instead of deleting and re-inserting, preserving the audit trail.
  - Clarified that the 11 seeded predicates in `predicate_constraints` are the complete set.

## 0.27.0 (2026-06-01)

### Added

- Preference system refactor (issue #53): behavioural index over the fact graph with contextual lookup and conflict resolution.
- New `Predicate::HasPreference = 11` seeded in `predicates` table.
- New lookup tables (re-seeded in migration 023):
  - `preference_categories`: 7 variants — CalendarBehavior, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, General.
  - `preference_source_types`: 3 variants — Interaction, Fact, UserEdit.
- New `PreferenceCategory` and `PreferenceSourceType` enums with `#[repr(i16)]` and `sqlx::Type`.
- New schema (migration 023):
  - `preferences` with `source_fact_id NOT NULL REFERENCES facts(id)`.
  - `preference_contexts` — normalized context conditions, no JSON.
  - `preference_sources` — provenance with `(preference_id, source_type_id, source_id)` unique constraint.
  - `preference_audit_log` — immutable history without FK to `preferences` (preserves history after deletion).
- Contextual lookup API: `get_preference(entity_id, key, query_context)` ranks by match count, confidence, and recency.
- Upsert API with conflict resolution:
  - Explicit overrides inferred.
  - Higher-confidence inferred wins.
  - Same confidence keeps existing.
  - `overridden_by_user = true` blocks inferred overwrites.
- Full audit logging on preference creation and overwrite.
- Source tracking for every preference.
- FK enforcement: non-existent `source_fact_id` is rejected.
- Comprehensive test suite in `mimir-knowledge/tests/preference_tests.rs` (15 tests).
- Technical documentation: `docs/preference-system.md`.
- User-facing documentation: `docs/wiki/preferences.md`.

### Changed

- **Breaking schema change:** old `preferences` and `preference_sources` tables dropped and recreated. No data migration attempted.

## 0.26.0 (2026-06-01)

### Added

- New built-in tool `get_weather` using wttr.in.
  - Fetches current conditions for any location (city name, airport code, or coordinates).
  - Returns structured JSON: temperature (°C/°F), feels-like, description, humidity, wind, UV index, visibility, and pressure.
  - Configurable base URL for testing (`GetWeatherTool::with_base_url`).

## 0.25.1 (2026-06-01)

### Fixed

- `get_active_facts_at` restored missing `AND fact_status_id = ?` filter so it again returns only active facts.
- `query_audit_log` switched from INNER JOINs to LEFT JOINs on `facts`, `entities`, and `predicates`, ensuring audit history remains visible after a fact is forgotten (hard-deleted).
- `mimir kb audit` now validates `--from` and `--to` datetime strings and exits with an error instead of silently ignoring malformed input.

## 0.25.0 (2026-06-01)

### Added

- Provenance audit refactor (issue #52): typed `change_type` and `changed_by` lookup tables with integer IDs.
- New lookup tables: `extraction_methods` (5 variants), `change_types` (7 variants), `changed_by_types` (4 variants).
- New `ExtractionMethod`, `ChangeType`, and `ChangedBy` enums with `#[repr(i16)]` and `sqlx::Type`.
- `mimir kb audit` CLI command for querying the fact audit log directly from the local SQLite database.
- `query_audit_log` API with filters: entity name, predicate name, datetime range, and change type.
- `add_source_to_fact` API for adding corroborating sources to an existing fact.
- `sources` unique constraint: `(fact_id, source_type_id, connector_id, raw_reference)`.
- Audit entries are now column-only JSON snapshots (e.g. `{"valid_until": ...}`) instead of full fact snapshots.

### Changed

- **Breaking schema change:** `source_types` remapped to 6 canonical variants: `UserEdit(1)`, `Connector(2)`, `Inference(3)`, `Interaction(4)`, `Import(5)`, `System(6)`. Old `Email`/`Calendar`/`Photo`/`Message` variants mapped to `Connector`; `CasualMention` mapped to `Interaction`.
- `fact_audit_log` recreated with `change_type_id`, `changed_by_id`, `reason`, and `changed_at` columns. Old action/performer strings migrated via best-effort mapping.
- `sources` recreated with `extraction_method_id INTEGER REFERENCES extraction_methods(id)`.
- `NewFact` expanded with `connector_id`, `connector_type`, `raw_reference`, and `extraction_method` fields.
- `update_fact_valid_until`, `update_fact_status`, and `forget_fact` now accept `ChangedBy` parameter.
- `forget.rs` deletes **all** `fact_dependencies` rows where the forgotten fact is parent or child (not just `InferredFrom`).
- Confidence cascade now writes `confidence_change` audit entries on child recalculation.

### Fixed

- Prevent duplicate edges when an already-superseded fact is superseded again by a third explicit fact.
- Correct `children` and `remaining_parents` queries in `forget.rs` after removal of relation_type filter from the DELETE query.

## 0.24.3 (2026-05-31)

### Added

- Structural confidence model (issue #51): confidence derived entirely from graph structure, zero LLM involvement, zero time-based decay.
- New `SourceType` variants: `CasualMention`, `Import`, `System`.
- New `ConnectorType` enum with SQLite lookup table and reliability tracking.
- `inference_confidence` formula: signed parent sum × chain penalty (0.8^depth) × breadth factor.
- `inference_depth` and `stale_confidence` columns on `facts` table.
- `is_positive` column on `fact_dependencies` for signed parent contributions.
- Per-connector reliability scores with feedback loop (`adjust_connector_reliability`).
- Eager bounded confidence cascade on parent removal.

### Changed

- `NewFact` no longer accepts caller-provided `confidence`; confidence is now computed in Rust (internal change; not public API).
- Connector-type source facts now use per-connector reliability scores instead of flat 0.80.
- Initial confidence values: `UserEdit`/`System` = 1.0, `CasualMention` = 0.30, `Import` = 0.80.

### Fixed

- Updated all test assertions and raw SQL to match new schema columns.

## 0.24.4 (2026-05-31)

### Fixed

- Build failure in `mimir-client`: replaced unsupported `reqwest` feature `rustls-tls-ring` with `rustls-native-certs` to align with `reqwest` 0.13 feature flags and `mimir-core` crate configuration.

### Documentation

- Added `docs/wiki/what-works-now.md`: comprehensive user-facing overview of all working features, current limitations, known bugs, and roadmap context.

## [0.33.0] - 2026-06-05

### Added

- **Category taxonomy system** (Dewey Decimal-style):
  - New `categories` table with hierarchical parent-child relationships.
  - `fact_categories` junction table allowing facts to belong to multiple categories.
  - Comprehensive seed taxonomy covering Identity (100), Food & Drink (200), Health (300), Relationships (400), Work (500), Home (600), Entertainment (700), Travel (800), and Schedule (900) with 2-3 levels of depth.
  - New KG tools: `expand_catalogue` and `get_facts_in_catalogue` for LLM-driven category browsing and fact retrieval.
  - System prompt injection of top-level catalogue so the LLM knows what knowledge domains exist.
  - CLI commands: `mimir kb category list`, `show`, `add`, `delete`.
  - Server routes: `GET /kb/categories`, `GET /kb/categories/{id}`, `POST /kb/categories`, `DELETE /kb/categories/{id}`.

- **Extraction pipeline category assignment**:
  - LLM suggests 1–3 category IDs per extracted fact via the `remember` tool.
  - Rust validates all suggested IDs against the database before insertion.

### Changed

- **Renamed `predicates` → `relationship_types`** and `predicate_constraints` → `relationship_constraints` across the entire codebase (DB schema, models, queries, tools, inference rules, tests).
- Updated all SQL queries, indexes, and foreign keys to use `relationship_type_id`.
- Updated `MemoryManager` and system prompt integration to read from the knowledge graph catalogue.

### Migration

- Migration `031_category_taxonomy_and_rename_predicates.sql` performs the rename and seeds the full category taxonomy.

## [0.34.0] - 2026-06-06

### Added

- **Issue #108**: Fact Ranking & Selection Engine (`mimir-knowledge`).
  - Introduced `memory_priorities` lookup table (Critical, High, Normal, Low) and `memory_priority_id` on `facts`.
  - Added `default_memory_priority_id` to `relationship_types` for automatic priority assignment at insertion.
  - Implemented scoring formula: `confidence × category.memory_weight × temporal_boost × priority_boost × centrality_boost`.
  - Temporal boost: `10.0 / sqrt(max(days, 0.5))` for future-dated facts (upcoming events, birthdays).
  - Centrality boost: entity connection count with in-memory `HashMap` cache, incrementally updated on mutation.
  - Budget fill algorithm: identity facts first (~200-char soft reservation), then greedy score-based fill to 2500-char limit.
  - Structured buckets: `identity`, `relationships`, `preferences`, `upcoming`, `general`.
  - Deterministic fallback renderer in Rust for when LLM condensation is unavailable.
  - `system_state` read/write queries for cached `condensed_memory`.
  - Unit and integration tests covering scoring, temporal boost, budget fill, renderer, and centrality cache.

## [0.38.1] — 2026-06-09

### Added

- **Issue #60**: Added explicit non-exhaustive note to context-injected system prompt.
  - When condensed memory is present, the system prompt now appends: "Note: This is not an exhaustive list. Use kg_query, kg_related, or kg_search tools if you need more information."
  - Signals the LLM that the injected memory is a curated subset, prompting tool use for deeper queries.
  - Completes the Layer 2 context injection design from Phase 2 Knowledge Graph architecture.
  - Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs`.
  - Updated unit tests, integration tests, and server integration tests to assert the note is present.
  - Updated documentation in `docs/personality-system.md`, `docs/wiki/personality.md`, and `docs/wiki/what-works-now.md`.
