# Calendar Connector (CalDAV) — `mimir-connectors::calendar`

> **Phase:** 3 — Connectors (C3 / issue #197, C4 / issue #198)
>
> **Feature flag:** `calendar` (default). Framework + mock stay built without it.
>
> **Status:** Implemented (library + daemon/CLI integration). C3 (#197) delivers transport + read/sync; C4 (#198) adds event → KB fact extraction, events-subsystem (#74) integration, and CalDAV write-back (`act`). Issue #247 propagates server-side deletions (tombstones) to the KB fact lifecycle; issue #314 makes the in-memory sync-token advance failure-safe (a cycle that fails after `sync` re-processes the staged window on the next in-process cycle). The daemon `AppState` wiring (A1 / #202), action routes (A2 / #203), the `mimir connector …` CLI (A3 / #204), and the interactive OAuth PKCE login (A4 / #205) are integrated.
>
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

The Calendar connector is the second concrete connector backend (after Photos). It syncs a CalDAV calendar collection into Mimir using the sync-token protocol and stages parsed VEVENTs for the knowledge-graph pipeline. It targets any RFC 5545 / RFC 6578 CalDAV server — Apple iCloud, Nextcloud, Fastmail, and (with caveats) Google — and runs in `Polling` mode.

C3 (#197) delivers the **transport**: PROPFIND (calendar verification) + sync-collection REPORT (incremental event fetch + sync-token cursor), OAuth token **refresh** + app-password auth, and `icalendar` VEVENT parsing. It stages parsed events in an internal buffer; **`extract()` returns no facts yet**. C4 / #198 converts those events into `NormalizedFact`s (events, locations, attendees → entities) and integrates them with the events & reminders subsystem (#74), and adds write-back (`act`).

## Auth

Two credential kinds, mirroring [`SecretBundle`](connector-secret-store.md):

- **App password** (iCloud, Fastmail, Nextcloud) — HTTP Basic auth. The username lives in `config_json` (non-secret); the password lives in the `SecretStore` under the connector slug as `SecretBundle::AppPassword`.
- **OAuth 2.0** (Google Calendar) — bearer token. The access/refresh tokens live in the `SecretStore` as `SecretBundle::OAuth`; the non-secret client config (`auth_uri`, `token_endpoint`, `client_id`, optional `client_secret`/`scopes`) lives in `config_json`. The connector **refreshes** an expired access token (within a 60 s skew) before every sync/authenticate/health call and persists the refreshed bundle back to the store. An **unknown** expiry (`expires_at: None`) does *not* force a refresh on every cycle — the token is reused and refreshed only once it is actually expired (avoiding triple POSTs to the token endpoint and rate-limit risk). When a refresh response omits `refresh_token` (RFC 6749 §6 allows this), the connector **retains** the prior refresh token, so OAuth does not break after the first refresh.

The token-endpoint error path reports only the HTTP status and the parsed `error`/`error_description` fields — the raw response body is never surfaced to `ConnectorError` strings (which the supervisor persists to `last_error` and logs) because provider error payloads can echo the `client_secret` or `refresh_token`. Auth-method/secret-kind mismatch errors use the auth-kind discriminant only, never a `Debug` of the OAuth config; both connectors build the error via the shared `secrets::mismatch_error` helper (issue #273), and the discriminant mapping behind it lives in the shared `secrets::AuthMethodDiscriminant` trait (issue #341).

The interactive PKCE login that *obtains* the first OAuth token is **A4 / `#205`**, implemented in `mimir-connectors::oauth::pkce` and driven by the CLI (`mimir connector add` / `auth` with an `auth.kind=oauth` config): the CLI binds an ephemeral loopback listener, opens the provider's authorize URL in the browser (printed first for headless sessions), exchanges the code, and POSTs the token bundle to the daemon's token-ingest route. `#197` consumes + refreshes a stored token.

## Sync protocol

One round trip per cycle (paged when truncated), via the [`CalDavClient`](#caldavclient):

1. `sync-collection` REPORT (RFC 6578) on the configured `calendar_url`. The request body carries the required `<d:sync-level>1</d:sync-level>` element (RFC 6578 §6.3), the `<d:sync-token>` (omitted for a full sync), and `<d:prop>` requesting `<d:getetag/>` + `<cal:calendar-data/>` inline so changed VEVENTs and a new `sync-token` arrive together (no follow-up multiget).
2. Omitting `<sync-token>` performs a **full** sync and yields the initial token; including it performs an **incremental** sync (no re-fetch).
3. The returned `sync-token` is the connector's incremental cursor. It is returned in `SyncOutcome` for the supervisor to persist via `KnowledgeGraph::update_sync_progress_and_durable_state`, and the connector adopts it as its in-memory marker only **after** the supervisor confirms the whole cycle succeeded (`Connector::on_cycle_succeeded`, issue #314) — never inside `sync`, so a cycle that fails after fetching re-syncs from the last confirmed cursor instead of skipping the failed window.
4. Each changed resource's `calendar-data` is parsed with `icalendar` into a `RawCalDavEvent` and staged. A `<response>` with no `calendar-data` is a tombstone **only** when its `<status>` is an explicit `404`/`410`; other statuses (`403` permission denied, `423` locked, `507` truncated, …) are logged and skipped so a transient server error never purges a live event. Tombstones are staged and propagated to the KB fact lifecycle (issue #247, below).
5. A truncated result set (RFC 6578 §6.5) is signalled with HTTP `507` (Insufficient Storage) carrying a partial multistatus body plus an advancing `sync-token`. The connector pages with the new token — re-issuing `sync-collection` and accumulating the partial changes — until a non-truncated response completes the sync.

`SyncOptions::full` ignores the persisted cursor (re-fetch everything).

The `roxmltree` XML helper concatenates *all* direct text/CDATA children of a leaf element (not just the first), so `calendar-data`/`summary`/`calendar-name` text split across multiple segments is not silently truncated.

## `CalDavClient`

`mimir_connectors::calendar::caldav::CalDavClient` wraps the workspace's `reqwest` 0.13 client and exposes:

- `sync_collection(url, sync_token)` → `SyncCollectionResult { new_sync_token, changed, deleted }`.
- `is_calendar(url)` → PROPFIND Depth 0 `resourcetype`; used by `health` and `authenticate` to verify the URL is a CalDAV calendar collection.
- `put_event(href, ical, etag)` → CalDAV `PUT` (RFC 4791 §5.5) for create (`etag = None` sends `If-None-Match: *`) or update (`etag = Some(t)` sends `If-Match: t`); returns the new `ETag`.
- `delete_event(href, etag)` → CalDAV `DELETE` (RFC 4791 §5.6); idempotent on `404`, `If-Match: t` when an etag is known.

WebDAV XML is parsed with `roxmltree` (a read-only DOM parser) matching element **local** names, so the varied namespace prefixes servers use (`D:`/`d:`/ `cal:`/`C:`) are tolerated. iCalendar payloads are parsed with `icalendar`'s low-level parser (`icalendar::parser::read_calendar`) — the high-level `icalendar::Calendar` is builder-oriented with no parse-from-str path in 0.17.x.

## Event → KB fact extraction (C4 / #198)

`CalendarConnector::extract` drains the staged VEVENTs into a cluster of `NormalizedFact`s, which the supervisor hands to the shared `normalize_and_insert` pipeline (entity resolution via F5, connector confidence, sensitivity gate, corroboration/supersession inherited). Per VEVENT it emits one primary fact, optionally one location fact, and one fact per attendee:

- **`user has_event <event>`** — the primary appointment. The subject is the canonical user identity (the `config.toml` `[identity] name`, injected via `ConnectorContext::user_identity` / `ConnectorSupervisor::with_user_identity`) so the event surfaces in the user's "Upcoming" memory section, which is scoped to the user entity. The object is an `Event` entity named by the `SUMMARY` (falling back to the `UID`). It carries the temporal bounds (`DTSTART`/`DTEND`), the recurrence (mapped from `RRULE` `FREQ`), and an `EventType::Appointment` hint so the events-subsystem (#74) overlay is typed correctly rather than defaulting to `Reminder`. When no user identity is configured the primary fact is skipped (the event is still captured via its location/attendee facts, but it will not appear in Upcoming).
- **`<event> located_in <place>`** — the `LOCATION` resolves to a `Place` entity via F5. The venue is a property of the event, not the user's location history, so this fact carries no `entity_locations` overlay (a calendar full of meetings would otherwise bloat `Visited` rows). It also carries no temporal bounds (`valid_from`/`valid_until`), so it spawns no events-subsystem overlay — only the primary `has_event` fact drives one.
- **`<attendee> attending <event>`** — each `ATTENDEE` resolves to a `Person` entity via F5 (the `CN` parameter, else the `mailto:` value). Like the location fact it carries no temporal bounds and spawns no events-subsystem overlay.

Dates are parsed to UTC at staging time: `DTSTART`/`DTEND` may be UTC (`…Z`), floating local, date-only, or `TZID`-qualified; the latter is resolved via `chrono-tz` (an unknown zone falls back to the naive value read as UTC so a bad `TZID` never drops the event). Only `RRULE` `FREQ` maps to the KB's coarse `RecurrenceType` (Daily/Weekly/Monthly/Yearly); `COUNT`, `UNTIL`, `INTERVAL`, and `BYxxx` are out of scope (the events-subsystem is a per-`FREQ` next-occurrence model, not a full RFC 5545 expander).

The `event_type: Option<EventType>` hint added to `NormalizedFact` is the mechanism: connectors that know the event kind supply it; chat leaves it `None` so the existing `Task`/`Reminder` derivation is unchanged.

## Server-side deletions (tombstones) → KB fact lifecycle (#247)

Deleting an event in another calendar client must not leave a phantom fact behind: `sync` already receives the deletion from the server's `sync-collection` window (a `404`/`410` response whose href is the deleted resource), and the connector now surfaces it to the knowledge graph:

1. **Staging** — `CalendarConnector::stage` moves each deleted href into a tombstone buffer instead of logging and dropping it.
2. **Raw reference identity** — the extractor authors every event fact's `raw_reference` as the CalDAV resource **href** (the server-side item id; previously the VEVENT `UID` with an href fallback). Sync-collection deletions report hrefs, so the tombstone maps 1:1 onto the facts.
3. **Trait surface** — `Connector::extract_deletions()` (new, default empty) reports the tombstone buffer **without draining it**; the supervisor calls it every cycle after `extract()` and hands the result to `KnowledgeGraph::forget_connector_facts_by_raw_reference(instance_id, raw_references, ChangedBy::System)`, then calls `Connector::acknowledge_deletions()` only after the cycle's trashing, fact insertion, and cursor persistence all succeeded (PR #313 review).
4. **KB trashing** — exactly the facts this instance authored with those raw references are trashed through the shared trash machinery (30-day recovery, inferred-child cascade, audit). The `events.fact_id` FK cascade-removes the events-subsystem overlay, so the event stops surfacing in "Upcoming" (one-time and recurring) and can never be advanced as an orphan. Idempotent: a tombstone reported twice trashes nothing the second time (mirroring `delete_event`'s 404-is-success semantics). A fact still corroborated by another connector or non-connector source is preserved — only its matching `sources` row is removed (PR #313 review).
5. **Cursor** — deletions arrive inside the sync-token incremental window, so no new cursor is needed. Pending tombstones are retained until the supervisor acknowledges them: a trash failure aborts the cycle before the cursor is persisted, the next in-process cycle re-reports the retained removals, and a restart resumes from the old sync-token and re-fetches the window (PR #313 review).

## Failure-safe cursor adoption (#314)

The in-memory sync-token must never run ahead of the persisted `connectors.sync_cursor`, which is updated only on a fully successful cycle. `CalendarConnector::sync` therefore does **not** advance its in-memory `sync_token`; the supervisor hands the persisted cursor back via `Connector::on_cycle_succeeded(new_cursor)` (new trait method, default no-op) only after the cycle's extraction, trashing, fact insertion, and cursor/durable-state persistence all succeeded. A cycle that fails after `sync` — an `extract` error, a trash error, or a hard `normalize_and_insert` error — leaves the in-memory marker at the last confirmed cursor, so the next in-process cycle re-syncs from it and the server re-reports the failed window's changed events (and deletions). This closes the gap that previously required a daemon restart (which re-seeds from the persisted cursor) or a manual full sync to recover a failed cycle's staged events. Tombstone staging dedupes by href, so repeated re-syncs of the same failed window do not grow the pending tombstone buffer. Acceptance: a failed extract/trash/insert cycle re-processes the staged changes on the next in-process cycle without a restart.

**Upgrade note (0.103.0):** facts authored before 0.103.0 carry the VEVENT `UID` as their `raw_reference` (with an href fallback), so href-based tombstones cannot match them and pre-upgrade deletions would leave phantom events active. The required cleanup is to remove the pre-upgrade facts of each Calendar instance (the connector-forget action trashes them, recoverable for 30 days) and trigger a full re-sync so the events are re-authored with href references — the compatibility boundary is deliberate: the href is the only server-side id that survives deletion.

The same trait + KB path is available to any connector whose service reports removals (the Email connector's iMIP `CANCEL` lifecycle is tracked separately as #283).

## Write-back (C4 / #198)

`CalendarConnector::act` is the only connector with write support. Three action kinds, each authenticated via the same credential path as `sync` (OAuth refresh included):

- `create_event` — builds a VEVENT from the payload (the `icalendar` builder), generates a `UID` (unless supplied), and `PUT`s it to `<calendar>/<uid>.ics` with `If-None-Match: *`.
- `update_event` — requires the target `href` (and optional `etag`); `PUT`s with `If-Match: <etag>`.
- `delete_event` — requires the target `href` (and optional `etag`); `DELETE`s it (idempotent on 404).

The `start`/`end` payload fields are RFC-3339 datetimes. `attendees` are bare addresses (an optional `mailto:` prefix is normalised). The returned `ActionResult::native_id` is the resource href; `message` carries the new `ETag` when the server supplies one. Every action `href` is validated against the configured `calendar_url` before the request is issued: the scheme, host, and port must match the configured origin and the path must lie under the calendar collection, so a caller-supplied URL cannot redirect the connector's stored credentials to another host or an unrelated resource on the same host.

## Secret-store injection

The Calendar connector is the first backend that needs credentials, so it extends the framework's `ConnectorContext` with a `secret_store: Option<Arc<dyn SecretStore>>` field and adds `ConnectorSupervisor::with_secret_store(store)`. The factory clones the store out of the context at construction; the connector loads its bundle by slug (the `__slug` the supervisor injects into `config_json`). This is a breaking change to an internal construction-context API, which the project's breaking-changes policy explicitly allows.

C4 (#198) extends the same context with the canonical **user identity name** (`ConnectorContext::user_identity` / `ConnectorSupervisor::with_user_identity`) so the connector authors `user has_event <event>` against the same entity the daemon resolves as `user_entity_id` (and the event surfaces in the user's "Upcoming" section). The Photos connector was aligned with the same shared identity in #246: it authors `took_photo_at` / `took_photo` facts against the injected identity, keeping `owner_name` only as a `None`-identity fallback.

## Dependencies

All optional, gated by the `calendar` feature:

| Crate | Version | Role |
|-------|---------|------|
| `icalendar` | 0.17.6 | Strongly-typed RFC 5545 iCalendar parser (default `parser` feature). **MSRV-capped at 0.17.6 by the workspace MSRV 1.85 — 0.17.7+ requires Rust 1.88; the Phase 3 deps ledger pins this resolution (issue #239).** |
| `roxmltree` | 0.21 | Pure-Rust read-only DOM XML parser for WebDAV multistatus responses. |
| `reqwest` | 0.13 (in tree) | HTTP (CalDAV + the `OAuthHttpClient` adapter's transport). |
| `oauth2` | 5.0.0 (`default-features = false`) | Vetted OAuth 2.0 protocol code (refresh grant + PKCE authorization-code flow, A4 / #205); talks HTTP through the `OAuthHttpClient` adapter over the workspace reqwest 0.13 client (issue #240). Gated by the `oauth` feature. |
| `chrono-tz` | 0.10 | IANA timezone database for `chrono`; resolves `TZID`-qualified `DTSTART`/`DTEND` to UTC. New in C4. |
| `uuid` | 1 (in tree) | `v4` UID generation for new write-back events. New in C4. |

OAuth token refresh runs on the vetted `oauth2` crate (issue #240): `oauth2` 5.0.0 is pulled with `default-features = false` so its optional reqwest 0.12 dependency never enters the tree, and a custom `OAuthHttpClient` adapter implements the crate's `AsyncHttpClient` trait over the workspace's single reqwest 0.13 client (see [OAuth client](oauth-client.md)). The adapter's client never follows redirects (a credential POST cannot be bounced to another host), the HTTPS/loopback endpoint gate is preserved, provider response errors surface only the parsed `error`/`error_description` fields (never the raw response body), and network failures include the underlying reqwest error detail.

## Config

Stored as `config_json` on the `connectors` row (with `__slug` / `__cursor` injected by the supervisor):

```json
{
  "calendar_url": "https://caldav.example.com/dav/devansh/personal/",
  "auth": { "kind": "app_password", "username": "devansh@example.com" },
  "poll_interval_secs": 900,
  "poll_jitter_secs": 60,
  "display_name": "Personal"
}
```

OAuth variant:

```json
{
  "calendar_url": "https://apidata.googleusercontent.com/caldav/v2/.../events/",
  "auth": {
    "kind": "oauth",
    "auth_uri": "https://accounts.google.com/o/oauth2/v2/auth",
    "token_endpoint": "https://oauth2.googleapis.com/token",
    "client_id": "mimir-client",
    "scopes": ["https://www.googleapis.com/auth/calendar.readonly"]
  }
}
```

## Tests

- Unit (`src/calendar/caldav/`): sync-collection full/incremental parse, 401 handling, PROPFIND resourcetype calendar detection, `icalendar` field extraction + recurrence, invalid-payload resilience — all against a `wiremock` mock CalDAV server.
- Integration (`tests/calendar_sync_tests.rs`): app-password sync, incremental sync-token, `full`-sync cursor reset, OAuth refresh-on-expiry + bundle persistence, health (online / not-configured / auth-expired), factory construction + config round-trip, and a full `ConnectorSupervisor` round-trip asserting the cursor is persisted on the connector row.

No `unsafe`; honours the workspace `#![deny(unsafe_code)]` guarantee.
