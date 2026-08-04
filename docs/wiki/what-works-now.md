# What Works in Mimir Today

> **Last updated:** 2026-08-04
> **Version:** 0.86.1
> **Release summary:** Phase 2 knowledge-graph work is live — core relationship ontology seeded category-first (Issue #135): predicate aliases for verb canonicalization plus `category_aliases` and category-subtree retrieval for grouping/multi-tag precision; relationship type aliases are the single source of truth for predicate resolution (Issue #133), Fact Ranking & Selection Engine (#108), LLM Condensation Pipeline & Regeneration Triggers (#109), live memory wired into the daemon, the `mimir-knowledge` forgetting system, Agentic Pre-Response Context Retrieval (#128), the Librarian Agent (#130), LLM-orchestrated learning via the `remember` tool (#137), a hardened system prompt that enforces the agentic contract — `retrieve_context` dispatch, no fact invention, and `remember` encouragement (#138), and a redesigned Librarian extraction prompt that injects the same core-facts block as the core agent and learns only from user-labelled messages (#139), and the full pending sensitive-fact confirmation lifecycle — HTTP routes, CLI commands, and a daily auto-cleanup job (#141). v0.57.0 adds the events & reminders subsystem — a lifecycle + recurrence overlay on facts that surfaces upcoming birthdays, appointments, deadlines, and tasks in the Upcoming memory section, with a deterministic scan job and the deprecation of `entity_dates` (#74).
> v0.85.0 adds Email structured extraction (Phase 3 C6 / #200): the IMAP Email connector's `extract()` now runs a deterministic extraction cascade over staged RFC 822 messages and, today, turns iMIP calendar invites (`text/calendar; method=REQUEST|REPLY`) into the same appointment fact cluster the Calendar connector emits — `user has_event <event>` typed `EventType::Appointment` (recurrence from `RRULE` `FREQ`, temporal bounds from `DTSTART`/`DTEND`), plus `<event> located_in <place>` and `<attendee> attending <event>` — reusing a new shared `mimir-connectors::ical` module (the Calendar connector's VEVENT parsing + fact cluster, now DRY across both backends). The email is treated as provenance (its IMAP UID is each fact's `raw_reference`), not the fact: no per-email communication facts and no `Person` entities auto-created from `From`/`To` headers, so marketing/spam produces no junk facts; a plain prose email with no `text/calendar` part produces nothing. The connector now authors user-scoped facts against the injected canonical user identity (`ConnectorContext::user_identity`), matching the Calendar connector. Free-text prose extraction (flights, bookings, confirmations) is C7 / #201.
> v0.86.0 adds Email schema.org JSON-LD deterministic extraction (Phase 3, #249): the IMAP Email connector's `extract()` cascade gains a second deterministic layer that scans `text/html` MIME parts for `<script type="application/ld+json">` blocks and extracts typed fact clusters for recognised `schema.org` types — `FlightReservation` (`user has_flight <flight>` typed `Appointment`, plus `departs_from`/`arrives_at`/`operated_by`), `LodgingReservation` (`has_booking`, `Appointment`, `located_in`), `EventReservation` (`has_event`, `Appointment`, `located_in`), `Order` (`has_order`, plus `purchased_from`), `ParcelDelivery` (`has_delivery` typed `Reminder`, plus `shipped_by`/`delivered_to`), `Ticket` (`has_ticket`, plus `issued_by`), and `ReservationPackage` (flattens `subReservation` for multi-leg flights). Unrecognised types are logged and skipped, never guessed. The primary user-scoped fact is only emitted when a canonical user identity is configured; secondary facts (airports, airlines, venues, carriers, merchants) are always emitted. No LLM — pure Rust parsing (no new dependencies). This is a library component in `mimir-connectors` (`email/jsonld.rs`) with unit tests for each type, the HTML `<script>` scanner, JSON-LD structural normalization, a cascade integration test (iMIP + JSON-LD from one email), and a KB integration test (flight fact entity resolution + `Appointment` overlay + connector provenance). The shared `vevent_fact` helper in `ical.rs` is made `pub(crate)` for DRY reuse.
> v0.86.1 addresses PR #257 review feedback on the JSON-LD extraction layer: the primary `Appointment`-typed reservation facts (flight/lodging/event) now require a parseable start time before emission (matching the iMIP layer's `DTSTART` rule, so no appointment overlay is created without a `valid_from`); `LodgingReservation` no longer emits a self-referential `located_in` when the resolved location equals the booking name; identifier fields (`orderNumber`/`trackingNumber`/`ticketNumber`/`flightNumber`/`iataCode`) now accept JSON numbers, and array-wrapped values are trimmed, via a shared `scalar_string` helper; `parse_datetime` accepts naive datetimes with fractional seconds and minute-only precision; `<script type="…">` attribute values are trimmed before the `application/ld+json` comparison (HTML5 spec compliance); and `mod jsonld` is narrowed to `pub(crate)`. A known limitation — iMIP + JSON-LD both firing on one email can produce duplicate `Event` entities — is now documented inline. New unit tests cover each change.
> v0.84.2 addresses PR #248 review feedback on the Calendar connector (C4 / #198): recurring `has_event` facts no longer carry the first instance's `DTEND` as `valid_until` (which would expire a weekly standup minutes after its first occurrence) — `valid_until` is left unset when `RRULE` `FREQ` is present, so current-facts reads and supersession keep the recurring fact live; CalDAV write-back (`create_event`/`update_event`/`delete_event`) now validates the payload `href` against the configured `calendar_url` origin (scheme, host, port, and calendar-collection path) before issuing the request, so a caller-supplied URL cannot redirect the stored credentials to another host or an unrelated resource on the same host; `TZID`-qualified datetimes at a DST autumn-fold prefer the earliest offset over the naive-as-UTC fallback; and the canonical user identity is stored trimmed so a padded `[identity] name` cannot create a duplicate person entity. Docs align with the new fact cardinality and the C4 status. This is a library component in `mimir-connectors`.
> v0.84.1 fixes the Calendar secondary-fact overlay (review follow-up, #198): `event_to_facts` no longer sets `valid_from`/`valid_until` on the secondary `located_in` and `attending` facts, which previously inherited the event's `DTSTART`/`DTEND` and spawned a spurious `Reminder` events-subsystem overlay; the primary `has_event` fact remains the sole overlay source.
> v0.84.0 adds Calendar event → knowledge-graph extraction and CalDAV write-back (Phase 3 C4 / #198): the CalDAV Calendar connector's `extract()` now drains staged VEVENTs into a cluster of `NormalizedFact`s through the shared `normalize_and_insert` pipeline — a primary `user has_event <event>` (typed `EventType::Appointment`, recurrence from `RRULE` `FREQ`), `<event> located_in <place>`, and `<attendee> attending <event>` — so every subject/object resolves to an entity via the full F5 chain and future-dated / recurring events surface in the user's "Upcoming" memory section (#74). The connector authors facts as the canonical user identity (the `config.toml` `[identity] name`, injected via the shared `ConnectorContext::user_identity`), matching the daemon's `user_entity_id` rather than carrying a disconnected owner-name. Dates are parsed to UTC at staging time, including `TZID`-qualified values resolved via `chrono-tz`; only `RRULE` `FREQ` maps to the KB's coarse `RecurrenceType`. C4 also adds the only connector write-back: `act()` creates/updates/deletes remote events via CalDAV `PUT`/`DELETE` (`If-None-Match` / `If-Match`, idempotent `DELETE` on 404), building VEVENTs with the `icalendar` builder. Server-side deletion → KB fact lifecycle is a follow-up (the extractor only yields facts). This is a library component with unit + integration tests (parsing, recurrence/date mapping, extraction shape, write-back against a mock CalDAV server, and a full sync → KB → "Upcoming" round-trip). New deps: `chrono-tz` 0.10 and `uuid` v1 (already in tree); `event_type: Option<EventType>` hint added to `NormalizedFact`.
> v0.83.0 adds the IMAP email connector (Phase 3 C5 / #199): the third concrete connector backend (after Photos and Calendar), in `mimir-connectors` (feature `gmail`). An `async-imap` 0.11.3 client (built `runtime-tokio`, no default `async-std`) speaks IMAP over a hand-rolled TCP + `tokio-rustls` handshake (the workspace keeps a single rustls TLS stack instead of async-imap's `connect()` / `async-native-tls`): `LOGIN` (app password) and `AUTHENTICATE XOAUTH2` (Google / Microsoft OAuth, with the access token refreshed via a shared hand-rolled token-endpoint POST — DRY with the Calendar connector, avoiding the reqwest-0.12-duplicating `oauth2` crate). It runs in `Push` (IMAP IDLE) mode when the server advertises IDLE and falls back to `Polling` otherwise — auto-detected via a CAPABILITY probe in `authenticate`/`health`. Incremental sync is by UID with a UIDVALIDITY-safe `<uid_validity>:<last_uid>` cursor (a mismatch on `EXAMINE` triggers a full re-fetch, so a recreated mailbox never silently gaps/duplicates), using `BODY.PEEK[]` so mail is not marked seen. The connector is transport-only: it logs in, watches for new mail, and stages raw RFC 822 messages; `extract()` returns no facts yet. Mail parsing + structured fact extraction (headers/dates/ contacts) is C6 / #200; LLM extraction (flights/bookings) is C7 / #201. The interactive OAuth PKCE login is A4 / #206; daemon `AppState` wiring and the `mimir connector …` CLI are A1–A3. This is a library component in `mimir-connectors` with unit tests plus a fake-IMAP integration suite (login, XOAUTH2 SASL, IDLE push, polling, incremental/no-op/full sync, UIDVALIDITY reset) over a `duplex` pair — no TLS, no live account. No new downloads (all deps already in the tree via reqwest/async-imap).
> v0.82.0 adds the CalDAV calendar connector (Phase 3 C3 / #197): the
> second concrete connector backend (after Photos), in `mimir-connectors`
> (feature `calendar`). A `CalDavClient` speaks CalDAV over the existing
> `reqwest` 0.13 — PROPFIND (Depth 0 `resourcetype`) for calendar/health
> verification and a `sync-collection` REPORT (RFC 6578) for event sync,
> requesting `<cal:calendar-data/>` inline so changed VEVENTs and a new
> `sync-token` arrive in one round trip. Omitting the sync-token does a full
> sync and yields the initial token; including it does an incremental sync
> (no full re-fetch), so the persisted sync-token is the connector's
> incremental cursor. `icalendar` parses each VEVENT (UID/summary/DTSTART/
> DTEND/location/status/RRULE) into a staged `RawCalDavEvent`; `roxmltree`
> parses the WebDAV XML by local tag name (namespace-prefix tolerant). Auth is
> an app password (HTTP Basic — iCloud/Fastmail/Nextcloud) or an OAuth bearer
> token (Google) that the connector **refreshes** when expired (within a 60 s
> skew) and persists back to the `SecretStore` — the interactive PKCE login
> that *obtains* the first token is A4 / #206. This is the first backend that
> needs credentials, so the framework `ConnectorContext` gained a
> `secret_store` field and `ConnectorSupervisor::with_secret_store` (a breaking
> internal construction-context change, allowed by policy). `extract()`
> returns no facts yet — C3 is transport-only; C4 / #198 does event → KB fact
> extraction + events-subsystem (#74) integration + write-back. This is a
> library component in `mimir-connectors` with unit + integration tests against
> a `wiremock` mock CalDAV server. It is available infrastructure; the daemon
> `AppState` wiring and `mimir connector …` CLI land in A1–A3.

> v0.78.0 adds the entity-locations write path (Phase 3 S3 / #193): a "where" fact (e.g. "I live at 10 Downing St") carries a typed `NormalizedLocation` overlay that `normalize_and_insert` turns into an `entity_locations` row for the resolved subject entity. The missing geo half is filled via the injected `Geocoder` — address-only is forward-geocoded to coords, coords-only is reverse-geocoded to a place name — and a move (home 2020–2023, home 2023–present) closes the prior open-ended location of the same type at the new start date. Rows link back to their source fact via a new nullable `source_fact_id` FK (migration `044`). The `Geocoder` is stored on `KnowledgeGraph` and injected by the daemon (Nominatim default); geocoder failures are logged and tolerated. The conversational `remember` tool schema gained an optional `location` object; connectors fill the same overlay field. Proximity queries (`find_nearby`) and the sensitive-fact confirmation path are follow-ups.
> v0.79.0 adds the entity-locations proximity query (Phase 3 S4 / #194):
> `KnowledgeGraph::find_nearby(lat, lon, radius_km, at)` returns every
> remembered location within a radius of a point, sorted nearest-first, each
> with its exact great-circle distance. A coarse SQLite bounding-box pre-filter
> (backed by a new composite `latitude, longitude` index, migration `045`) is
> followed by an exact Haversine post-filter computed in pure Rust
> (`mimir-knowledge::geo` — no external `geo` crate). An optional `at` instant
> scopes results to locations valid at that time. This closes the query half of
> #65 (the write half landed in #193). Locations without coordinates are
> skipped; edge-of-box over-inclusions are dropped by the exact distance.

> v0.80.0 adds the first concrete connector backend — the local-filesystem
> Photos connector (Phase 3 C1 / #195): a read-only, push-mode, no-network
> connector in `mimir-connectors` (feature `photos`) that watches a configured
> directory recursively with `notify` (debounced ~2s), extracts EXIF GPS +
> datetime with `kamadak-exif` (JPEG/TIFF/HEIF/PNG/WebP), and emits one
> `took_photo` fact per photo (C1) / `took_photo_at <place>` fact (C2 / #196,
> v0.81.0) through the shared `normalize_and_insert` pipeline. C2
> reverse-geocodes the EXIF GPS into a locality-level place name (reusing the
> shared `Geocoder` injected via a new `ConnectorContext` threaded factory →
> registry → supervisor) so the place is a `Place` object entity and photos at
> the same spot corroborate into one open-ended fact (+0.05/source, capped
> 0.95; base 0.80). A coord-dedup cache (~111 m buckets) bounds geocode calls
> to one per shooting spot, and transient errors aren't cached. Two
> `entity_locations` rows are written: the owner's `Visited` row (coords +
> place name) and a new idempotent `Geographic` row (migration `046`) anchoring
> the place entity's own coordinates, so `find_nearby` resolves places by
> where they are. When no place resolves, the photo degrades to the C1
> coords-only `took_photo` shape so no data is lost. A per-file mtime/inode
> incremental cursor persists across restarts so unchanged photos are never
> re-scanned; the supervisor injects the persisted `sync_cursor` into a
> connector's `config_json` as `__cursor`. This is a library component; the
> daemon `AppState` wiring and `mimir connector …` CLI land in A1–A3.
> v0.81.0 adds Photos connector GPS → place extraction (Phase 3 C2 / #196): the local-filesystem Photos connector (C1 / #195) now reverse-geocodes each photo's EXIF GPS into a locality-level place name via the shared `Geocoder` (injected through a new `ConnectorContext` threaded factory → registry → supervisor), emitting `owner took_photo_at <place>` facts whose place is a `Place` object entity. Photos at the same place corroborate into one open-ended fact (+0.05/source, capped 0.95; base confidence 0.80), so the knowledge graph grows with distinct places visited, not photo count. A coord-dedup cache (~111 m buckets) bounds geocode calls to one per shooting spot; transient errors aren't cached. Two `entity_locations` rows are written per place fact: the owner's `Visited` row (coords + place name) and a new idempotent `Geographic` row (migration `046`, `LocationType::Geographic = 6`) anchoring the place entity's own coordinates, so `find_nearby` resolves places by where they are. `GeocodeResult` gained a `short_name` field (the most specific locality: city → town → village → … → first display-name segment). When no place resolves (no geocoder / no match / transient error), the photo degrades to the C1 coords-only `took_photo <rel_path>` shape so no data is lost. This is a library component with unit + integration tests; the daemon `AppState` wiring (A1) and `mimir connector …` CLI (A3) land in later Phase 3 issues.
> 

> v0.77.0 adds the geocoder service (Phase 3 S1 / #191): a pluggable `Geocoder` trait (forward address → coords, reverse lat/lon → place) with an OSM Nominatim default backend. The trait and `GeocodeResult`/`GeocodeError` types live in `mimir-core` (so the Location Search tool #98 — a `mimir-core` tool — can name it; `mimir-core` cannot depend on `mimir-connectors`), and the `NominatimGeocoder` backend lives in `mimir-connectors`. Throttling reuses the F12 `RateLimiter` (`RateLimitConfig::nominatim`, ≤ 1 req/s) and transient 429/502/503/504 + transport failures retry via `retry_with_backoff` honouring a `Retry-After`; quota exhaustion is non-retryable. The endpoint, descriptive `User-Agent` (Nominatim policy), optional contact email, rate-limit policy, and retry budget are all configurable (self-hosted Nominatim is supported for heavy use). A successful "no match" yields `Ok(None)`; transport/decode failures yield `Err(GeocodeError)` and are logged — they never panic. Results carry lat/lon/country/`country_code`/alternative names. This is a library component with `wiremock`-backed integration tests; wiring into the Photos connector (C2), the entity-locations write path (S3/#65), and the Location Search tool (#98) lands in later Phase 3 issues.

> v0.72.0 adds shared rate-limiting + retry/backoff primitives for network connectors (Phase 3 F12 / #189): a `RateLimitConfig` (`requests_per_second` / `burst_size` / optional `daily_quota` / `backoff_strategy`) plus a `RateLimiter` (a vetted, `unsafe`-free `governor` GCRA token bucket) and a `retry_with_backoff` helper. Connectors will route their outbound HTTP/IMAP/CalDAV API calls through one per-instance limiter for uniform throttling, an optional rolling 24h daily cap (which returns a non-blocking `QuotaExhausted` so the supervisor can pause gracefully instead of parking a task), and 429/502/503/504 retry with exponential/linear/fixed backoff + jitter honouring a server `Retry-After`. Connector **LLM** calls are exempt (decision D′) — those route through the shared `LlmWorkerPool` system queue. The config is `serde`-serialisable (human-readable durations) so it embeds in each connector's `config_json`; a `nominatim()` preset enforces the OSM Nominatim ≤ 1 req/s policy. This is a library component in `mimir-connectors` with unit + integration tests. It is available infrastructure now; connectors will route their outbound calls through it as their backends land (geocoder #191, Photos/Calendar/Email backends in later Phase 3 issues), and the rolling daily-quota window can be snapshotted and restored across restarts so a relaunch cannot bypass a provider's 24-hour quota.

> v0.75.0 adds the configurable, always-compiled mock connector test harness (Phase 3 F13 / #190): an in-memory connector (`MockConnector` + `MockConnectorFactory` + `MockFactConfig` + `MockSyncRecorder`) whose behaviour is driven entirely by its `config_json` — it emits canned `NormalizedFact`s on a configurable cadence in both `Polling` and `Push` modes, with configurable health/auth state, failure/panic injection, an optional `batch_size` for incremental sync, and sync-options observation for concurrency tests. `MockConnector::default()` preserves the legacy no-op identity so existing trait tests keep passing. It is the T1 sync→extract→insert→query vehicle: the real `ConnectorSupervisor` + `KnowledgeGraph` ingest a mock's canned facts end-to-end with connector provenance (`SourceType::Connector`, `connector_instance_id`, `raw_reference`, `ExtractionMethod::StructuredParse`), without any real service. The previous private `TestConnector` in the supervisor lifecycle tests was removed in favour of the shared `MockConnector` (DRY). No new dependencies. This is a library component in `mimir-connectors` with unit + integration tests.

> v0.71.0 adds the connector secret store (Phase 3 F10 / #187): a single `SecretStore` trait backs every connector auth kind — one `SecretBundle` enum covers OAuth 2.0 (`access_token` + optional `refresh_token` + optional `expires_at`), API tokens, and app passwords, keyed by connector slug. The V1 default `FileSecretStore` persists one JSON file per connector under `~/.local/share/mimir/secrets/<slug>.json`, file mode `0600`, directory `0700`, plaintext at rest (consistent with the plaintext LLM API key in `config.toml` and the home-directory trust boundary; at-rest encryption deferred). Loads *fail closed*: a secret file or directory with any group/other permission bits set is refused rather than read, writes are atomic (temp + rename), and slugs are validated against `[A-Za-z0-9_-]{1,128}` to block path traversal. An `InMemorySecretStore` is included as a test/helper backend. The end-to-end `connector remove` secret wipe is the consumer's job (server/CLI routes, #202/#204/#203); this issue delivers the `delete(slug)` capability.

> v0.60.0 adds corroboration detection (#79): when a new non-explicit fact covers the same claim as an existing Active or pending_confirmation fact (same subject + predicate + object, temporally overlapping), Mimir adds a source to the existing fact instead of creating a duplicate, and boosts its confidence +0.05 per independent source (capped at 0.95; explicit and inferred facts excluded). Re-statements from the same source are a no-op, and the confidence change cascades comprehensively to inferred children.

> v0.65.0 adds the shared `normalize_and_insert` ingestion boundary (Phase 3 F4 / #181): the resolve → confidence → sensitivity-gate → insert orchestration is extracted from the conversational `remember` path into one reusable function in `mimir-knowledge::normalize`. Both chat learning and (future) service connectors funnel through it via a provenance-annotated `NormalizedFact` type and a batch-level `Provenance`, so connector-sourced facts get identical confidence scoring, corroboration, supersession, and sensitivity gating — including cross-connector corroboration, where a Gmail flight fact and a Calendar event describing the same trip merge into one knowledge-graph fact with boosted confidence instead of duplicating.

> v0.66.0 adds the full entity-resolution chain (Phase 3 F5 / #182): `resolve_entity` now runs exact name → alias → FTS5 fuzzy (score ≥ 0.9) → create new, restricted to the requested entity type. A short token-overlap query like "John" resolves to the canonical "John Smith" person, while a cross-type fuzzy hit ("Apple" as a concept vs "Apple Inc" the organization) is dropped so a new entity is created instead of a wrong merge. The chain is shared by chat extraction and connectors; alias learning stays explicit via `preferred_name`.

> v0.67.0 defines the runtime `Connector` trait and its data types (Phase 3 F6 / #183): the async, object-safe `Connector` interface every service-ingestion worker implements — `sync` (fetch raw items) → `extract` (produce typed `NormalizedFact`s), plus `authenticate`, `health`, optional `act` write-back, and `forget`. Ingestion is two-step and DB-free: the connector fetches and parses, and the supervisor (F8) will call the shared `normalize_and_insert` pipeline. New types include `ConnectorMode` (polling vs push), `SyncOptions`/`SyncOutcome`, `HealthStatus` (a transient probe, renamed to disambiguate from the persisted lifecycle enums), `ConnectorAction`/`ActionResult`, and `ConnectorError`. No backends sync yet.

> v0.68.0 adds the `ConnectorRegistry` and multi-backend factory dispatch (Phase 3 F7 / #184): the registry maps each `(connector_type, backend)` pair — e.g. `(Email, imap)` or `(Calendar, caldav)` — to a `ConnectorFactory` that constructs the right implementation from a connector's stored config. A connector *type* is the reliability/provenance axis; a *backend* is the provider implementation chosen per instance. New backends register a new factory with no schema change, many backends coexist under one type, and reliability stays per-type. A closure-backed `FnConnectorFactory` and an always-compiled `MockConnectorFactory` keep the registry exercisable under every feature combination. The supervisor, secret store, and concrete backends (Photos, CalDAV Calendar, IMAP Email) land in later Phase 3 issues.

> v0.69.0 adds the `ConnectorSupervisor` supervised lifecycle (Phase 3 F8 / #185): one supervised background task per connector whose status is `Active`, centralising spawn-on-startup, restart with exponential backoff, a circuit breaker (after `max_failures` consecutive failures the connector moves to `Error` and stops auto-restarting, requiring a manual `resume`), auth-expiry pausing (`health() == AuthExpired` → `auth_state = Expired`, `status = Paused`, task exits), graceful shutdown, and cursor persistence. `Paused` / `Error` / `Setup` connectors are not auto-started. Each cycle runs `health` → `sync` → `extract` → `normalize_and_insert` (the shared ingestion boundary) in an isolated sub-task so a connector panic is caught via `JoinError::is_panic` instead of unwinding the runner; the shared shutdown `watch` channel aborts in-flight cycles, with the cursor always reflecting the last completed sync (daemon/CLI wiring that drives this channel from `mimir stop` lands in later Phase 3 issues). `yield-on-user-activity` is deferred for V1. This is a library component in `mimir-connectors` with integration tests against a configurable in-memory mock; daemon `AppState` wiring and the `mimir connector …` CLI land in later Phase 3 issues (A1–A3). The secret store and concrete backends (Photos, CalDAV Calendar, IMAP Email) land in later Phase 3 issues.

> v0.70.0 adds manual sync triggering (Phase 3 F9 / #186): `ConnectorSupervisor::trigger_sync(id, SyncOptions)` (and a slug-based `trigger_sync_by_slug`) wakes a connector's runner from its polling-interval wait so a sync runs immediately with caller-supplied options — `--full` forces a non-incremental pass (cursor ignored/reset) and `since` is a relative time-window hint. A one-permit `tokio::sync::Semaphore` per connector serialises concurrent callers (overlapping triggers queue rather than launching parallel cycles), and a per-connector request channel carries the options and returns the cycle's `TriggerOutcome` (`Ok { fetched, new_cursor }`, `AuthExpired`, or `Failed`). Triggering a connector that is not running (`Paused`/`Error`/`Setup` or exited) returns `TriggerError::NotRunning`; push-mode connectors (no polling interval to preempt) return `TriggerError::PushUnsupported` — push manual sync is deferred. The runner's post-cycle wait is now a `select!` between the polling interval, a trigger, and shutdown, so a trigger preempts the interval (and backoff after a failure). This is a library component in `mimir-connectors` with integration tests against a configurable in-memory mock; daemon `AppState` wiring and the `mimir connector sync …` CLI land in later Phase 3 issues (A1–A3). The secret store and concrete backends (Photos, CalDAV Calendar, IMAP Email) land in later Phase 3 issues.


---

## What Is Mimir?

Mimir is a **persistent, personal intelligence** that runs as a local daemon on your machine. It is not a chatbot — it is a stateful companion that remembers facts, preferences, and conversation history across sessions, and becomes more useful the longer you use it.

Key design principles:

- **Local-first** — All data stays on your device. No cloud intermediary.
- **Persistence over ephemerality** — Every interaction is stored, versioned, and retrievable.
- **User sovereignty** — You can inspect, edit, and delete anything Mimir knows.
- **OpenAI-compatible** — Works with any local or remote endpoint that speaks the OpenAI chat completions API.

---

## Architecture at a Glance

Mimir is distributed as a **single binary** (`mimir`) that operates in two modes:

| Mode | Command | What it does |
|------|---------|--------------|
| **Daemon** | `mimir start` | Runs an Axum HTTP server on `127.0.0.1:8080` |
| **Client** | `mimir ask`, `mimir chat`, etc. | Talks to the daemon via HTTP |

Library crates provide code organisation:

- `mimir-core` — LLM client, config, memory, context, personality, tools, skills, paths
- `mimir-server` — Axum routes, state, middleware (library, no binary)
- `mimir-client` — HTTP client for talking to the daemon
- `mimir-api-types` — Shared request/response types
- `mimir-knowledge` — SQLite knowledge graph (Phase 2; wired into daemon via live memory block and condensation pipeline)
- [`Librarian Agent`](../../docs/librarian-agent.md) — On-demand fact-extraction agent; no longer auto-triggered every turn (see #137). Its extraction prompt now reuses the core agent's core-facts block and learns only from `[User]`-labelled messages (#139)

---

## Quick Start

### 1. Build

```bash
cargo build --workspace --release
```

### 2. Initialise

```bash
./target/release/mimir init
```

This creates:
- `~/.config/mimir/config.toml`
- `~/.local/share/mimir/` (data directory)

### 3. Configure

Edit `~/.config/mimir/config.toml`:

```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-4o"
temperature = 0.2

[server]
bind_addr = "127.0.0.1:8080"

[personality]
preset = "transparent"

[memory]
char_limit = 2500
```

Environment variables override config values (e.g. `MIMIR_LLM_API_KEY`).

### 4. Run

```bash
# Terminal 1 — start the daemon
mimir start

# Terminal 2 — ask a question
mimir ask "What is the capital of France?"

# Or start an interactive chat
mimir chat
```

If the daemon is not running, client commands will prompt you to auto-start it.

---

## Feature Reference

### CLI Commands

All client commands talk to the daemon over HTTP. If the daemon is down, you are prompted to start it (unless stdin is not a TTY).

| Command | Status | Description |
|---------|--------|-------------|
| `mimir init` | ✅ Works | First-run bootstrap: creates directories, default config, and optionally installs a systemd user service |
| `mimir start` | ✅ Works | Runs the daemon in the foreground (binds to TCP localhost) |
| `mimir stop` | ✅ Works | Graceful shutdown via POST `/stop` |
| `mimir ask` | ✅ Works | Single-shot query with streaming, piping, model/personality override, incognito mode, and verbose token usage |
| `mimir chat` | ✅ Works | Interactive REPL with session history, `/history` resume, `/memory`, `/status`, `/clear`, `/help`, multi-line input, and SSE streaming |
| `mimir status` | ✅ Works | Health check: config, LLM reachability, queue depth, memory usage |
| `mimir memory` | ✅ Works | Prints the live condensed memory block from the knowledge graph |
| `mimir tool list` | ✅ Works | Lists registered tools and their permissions |
| `mimir tool enable/disable/permission` | ✅ Works | Change tool permission levels (saved to `tools.toml`) |
| `mimir skill list/show/add/delete/enable/disable` | ✅ Works | Manage skills (built-in, user-added, and generated) |
| `mimir kb` | ✅ Works | All `mimir kb` commands route through daemon HTTP; audit and CRUD supported via daemon |

### Chat & Conversation

| Feature | Status | Notes |
|---------|--------|-------|
| Streaming responses | ✅ Works | SSE `/chat/stream` endpoint; tokens arrive in real time |
| Non-streaming responses | ✅ Works | `/chat` endpoint; full response returned as JSON |
| Session persistence | ✅ Works | Each conversation gets a UUID; history is SQLite-backed |
| Session resume | ✅ Works | `/history` in `mimir chat` lets you pick and resume past sessions |
| Context trimming | ✅ Works | Automatically trims to `max_tokens` and `max_turns` config limits |
| Conversation history search (FTS5) | ✅ Works | `search_conversation_history` built-in tool with snippet extraction |
| Incognito mode | ✅ Works | `--incognito` skips all persistence (no session, no memory learning) |
| Model override | ✅ Works | `-m gpt-4o-mini` creates a cached override client |
| Personality override | ✅ Works | `-p concise` overrides the config preset for one query |
| Markdown rendering | ✅ Works | Terminal output adds blank lines around code fences for readability |
| Piped input | ✅ Works | `cat file.txt \| mimir ask …` |
| Multi-line input | ✅ Works | Ctrl-D to submit multi-line text in interactive chat |
| Token usage display | ✅ Works | `--verbose` shows prompt/completion/total token counts |

### Tools & Skills

| Feature | Status | Notes |
|---------|--------|-------|
| Tool registry | ✅ Works | Object-safe `Tool` trait; permissions per tool |
| Skill registry | ✅ Works | Object-safe `Skill` trait with `SkillContext` |
| Builtin tools | ✅ Works | `get_current_time`, `search_web`, `memory`, `context_summary`, etc. |
| Builtin skills | ✅ Works | `research_synthesis`, `test_driven_development` |
| User skills | ✅ Works | Markdown files in `~/.config/mimir/skills/` |
| Generated skills | ✅ Works | Auto-created by the agent; tracked with metrics |
| Metrics tracking | ✅ Works | SQLite-backed invocation counts, success rates, corrections |

### Memory System

| Feature | Status | Notes |
|---------|--------|-------|
| Knowledge graph memory | ✅ Works | Live condensed memory (~2,500 chars) ranked from the knowledge graph and injected into every system prompt |
| LLM-orchestrated learning | ✅ Works | The LLM calls the `remember` tool during conversation to persist facts; learning no longer fires automatically on every turn (#137) |
| Frozen snapshots | ✅ Works | Condensed memory is read from `system_state` once per session; changes don't affect the current chat |
| Knowledge-graph managed | ✅ Works | Manage memory via the knowledge-graph UI/CLI or import/export tools; no memory.md file |
| Size limit enforcement | ✅ Works | Configurable `char_limit` (default 2,500) |

### Configuration

| Feature | Status | Notes |
|---------|--------|-------|
| TOML config file | ✅ Works | `~/.config/mimir/config.toml` |
| Environment overrides | ✅ Works | `MIMIR_LLM_API_KEY`, `MIMIR_BASE_URL`, etc. |
| XDG path resolution | ✅ Works | Respects `XDG_CONFIG_HOME` and `XDG_DATA_HOME` |
| Hot-reload | ✅ Works | Non-sensitive config changes apply without restarting the daemon |
| Auto-initialisation | ✅ Works | First use creates defaults automatically |

### Personality

| Feature | Status | Notes |
|---------|--------|-------|
| Presets | ✅ Works | `transparent`, `concise`, `warm`, `formal` |
| System prompt generation | ✅ Works | Combines preset + condensed memory from the knowledge graph; explicitly marked as non-exhaustive with a note directing the LLM to KG tools |
| CLI override | ✅ Works | `--personality` flag on `mimir ask` |

### Deployment & Operations

| Feature | Status | Notes |
|---------|--------|-------|
| systemd user service | ✅ Works | `mimir init` offers to install and enable it |
| Graceful shutdown | ✅ Works | `mimir stop` or Ctrl-C / SIGTERM |
| Daemon-down detection | ✅ Works | CLI probes `/status`; prompts to start if unreachable |
| Loopback security | ✅ Works | `/stop` is restricted to `127.0.0.1` |
| CORS for local dev | ✅ Works | Whitelisted ports: 8080, 3000, 5173 |

### Knowledge Graph (Phase 2)

| Feature | Status | Notes |
|---------|--------|-------|
| SQLite schema & migrations | ✅ Works | In `mimir-knowledge` crate |
| Entity CRUD | ✅ Works | Types, aliases, deduplication, dates, locations (stubs) |
| Fact CRUD | ✅ Works | Temporal bounds, statuses, dependencies, cascade forget |
| Confidence model | ✅ Works | Graph-derived; no LLM involvement, no decay |
| Inference engine (Rust) | ✅ Works | Transitivity, contradiction, propagation, threshold rules |
| Provenance tracking | ✅ Works | Source tracking with connector_instance_id FK + raw_reference + typed audit log with change_type/changed_by |
| Forgetting system | ✅ Works | Trash, cascade forget, restore, bulk operations |
| FTS5 search | ✅ Works | Full-text search over entities and aliases |
| **Fact extraction pipeline** | ✅ Works | LLM → Rust validation → entity resolution (exact → alias → FTS5 fuzzy ≥ 0.9, type-filtered → create) → confidence → sensitive confirmation → insert (issues #55, #182) |
| **`mimir kb` CLI (daemon-routed)** | ✅ Works | All `mimir kb` commands route through daemon HTTP (no direct DB access); audit and CRUD supported via daemon |
| **Pending sensitive-fact confirmation** | ✅ Works | `GET /kb/pending`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject`; CLI `mimir kb pending|confirm|reject`; optional reject `--reason` written to the audit log (#141) |
| **Pending-fact auto-cleanup** | ✅ Works | Daily `knowledge.pending_cleanup` job hard-deletes facts awaiting confirmation past `retention_days` (default 7); configurable under `[knowledge.pending_cleanup]` (#141) |
| **Relationship type DAG + aliases** | ✅ Works | `relationship_type_hierarchy` and `relationship_type_aliases` tables enable ontology-driven predicate discovery; aliases resolve automatically through `ensure_relationship_type` |
| **Category aliases + subtree retrieval** | ✅ Works | `category_aliases` map domain words (`education`, `hobbies`, `family`…) to Dewey categories; `get_facts_in_category_subtree` gathers facts across a category subtree (#135) |

---

## API Endpoints

The daemon exposes an OpenAI-compatible chat endpoint plus Mimir-specific management endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/status` | Health, config, LLM reachability, memory usage |
| `GET` | `/memory` | Live condensed memory block from the knowledge graph |
| `GET` | `/sessions` | List conversation sessions |
| `GET` | `/sessions/{id}/messages` | Messages for a session (from last compaction) |
| `POST` | `/chat` | Blocking chat with agentic tool loop |
| `POST` | `/chat/stream` | SSE streaming chat |
| `POST` | `/stop` | Graceful shutdown (loopback only) |
| `GET` | `/kb/query` | Query facts for an entity |
| `GET` | `/kb/facts/{id}` | Show a single fact with sources, deps, audit |
| `PATCH` | `/kb/facts/{id}` | Edit mutable fact fields |
| `POST` | `/kb/facts/forget` | Forget facts (single or bulk) |
| `GET` | `/kb/browse` | Graph traversal from an entity |
| `GET` | `/kb/profile` | Generate entity profile from top-confidence facts |
| `GET` | `/kb/audit` | Query the fact audit log |
| `GET` | `/kb/trash` | List trash contents |
| `POST` | `/kb/trash/restore` | Restore facts from trash |
| `DELETE` | `/kb/trash` | Empty trash permanently |
| `GET` | `/kb/pending` | List sensitive facts awaiting confirmation |
| `POST` | `/kb/facts/{id}/confirm` | Confirm a pending fact (→ Active, confidence 1.0) |
| `POST` | `/kb/facts/{id}/reject` | Reject a pending fact (hard-delete + audit; 204) |

---

## Known Limitations & Open Issues

| Issue | Impact | Workaround |
|-------|--------|------------|
| [#71](https://github.com/BhavsarDevansh/Mimir/issues/71) — `mimir chat` streaming bug | Streaming may fail in some environments | Use `mimir ask` for single-shot queries; restart daemon if stream stalls |
| [#45](https://github.com/BhavsarDevansh/Mimir/issues/45) — UTC time | `get_current_time` returns UTC | Ask Mimir to convert to your timezone verbally |
| [#25](https://github.com/BhavsarDevansh/Mimir/issues/25) — Unix socket transport | TCP is the only transport | TCP on `127.0.0.1:8080` is secure for local use |
| | | 

---

## Roadmap Summary

- **Phase 1 — Core Agent** ✅ Complete
- **Phase 2 — Knowledge Graph** ✅ Complete
- **Phase 3 — Connectors** 🚧 In progress — the `mimir-connectors` crate is scaffolded (crate, feature flags `photos`/`calendar`/`gmail`, DB-access boundary via `KnowledgeGraph` only), the `connectors` instance-registry table + `KnowledgeGraph` facade methods landed in #179 / F2 (sync cursor, auth state, and health persist across restarts), the `sources.connector_instance_id` provenance FK migration + per-connector item-count query landed in #180 / F3, the shared `normalize_and_insert` ingestion boundary landed in #181 / F4 (connectors funnel through the same confidence/corroboration/sensitivity pipeline as chat), the full entity-resolution chain landed in #182 / F5, and the runtime `Connector` trait + data types landed in #183 / F6 (the async, object-safe contract every connector implements; two-step DB-free ingestion — `sync` → `extract` → supervisor-owned `normalize_and_insert`), and the `ConnectorRegistry` + multi-backend factory dispatch landed in #184 / F7 (the registry maps `(connector_type, backend)` to a `ConnectorFactory`; new backends register a new factory with no schema change, many backends coexist under one type, and reliability stays per-type), and the `ConnectorSupervisor` supervised lifecycle landed in #185 / F8 (one supervised task per `Active` connector: spawn-on-startup, restart-with-backoff, circuit breaker, auth-expiry pausing, graceful shutdown, and cursor persistence; each cycle runs `sync` → `extract` → `normalize_and_insert` in an isolated sub-task so a connector panic is contained). The connector secret store landed in #187 / F10 (a single `SecretStore` trait + `SecretBundle` enum + `FileSecretStore`: one `0600` JSON file per connector under `~/.local/share/mimir/secrets/`, plaintext at rest, fail-closed permission checks, atomic writes, slug validation; an `InMemorySecretStore` helper is included). The shared rate-limit + retry/backoff primitives landed in #189 / F12 (a per-instance `RateLimiter` backed by `governor` GCRA + an optional rolling 24h daily quota that returns a non-blocking `QuotaExhausted`, plus a `retry_with_backoff` helper for uniform 429/502/503/504 handling with exponential/linear/fixed backoff + jitter and `Retry-After` honouring; connector LLM calls are exempt per decision D′). The configurable, always-compiled mock connector test harness landed in #190 / F13 (a config-driven in-memory connector emitting canned `NormalizedFact`s in polling/push modes with health/auth/failure/panic injection; the T1 sync→extract→insert→query vehicle; the supervisor lifecycle tests now drive it, replacing the private `TestConnector`). The daemon wiring and optional OS-keyring backend (#188) land in later Phase 3 issues; the Photos (C1/C2) and CalDAV Calendar (C3) concrete backends have already landed, with email and file-watchers backends still to come
- **Phase 4 — Reasoning** ⏳ Planned (inference engine expansion)
- **Phase 5 — Proactive Agent** ⏳ Planned (events, reminders, domain surfacing)
- **Phase 6 — Vision** ⏳ Planned (long-term memory consolidation)

See `VISION/09-Roadmap/` for full details.

---

## Getting Help

- Read the per-feature wiki docs in `docs/wiki/` for deep dives on individual subsystems.
- Check the GitHub Issues board for bug reports and feature requests.
- Run `mimir status` to verify daemon health and configuration.
