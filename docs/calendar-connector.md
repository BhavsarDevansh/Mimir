# Calendar Connector (CalDAV) — `mimir-connectors::calendar`

> **Phase:** 3 — Connectors (C3 / issue #197, C4 / issue #198)
> **Feature flag:** `calendar` (default). Framework + mock stay built without it.
> **Status:** Implemented (library + daemon/CLI integration). C3 (#197) delivers transport + read/sync; C4 (#198) adds event → KB fact extraction, events-subsystem (#74) integration, and CalDAV write-back (`act`). The daemon `AppState` wiring (A1 / #202), action routes (A2 / #203), and the `mimir connector …` CLI (A3 / #204) are integrated; only the interactive OAuth PKCE login remains (A4 / #205). Server-side deletion → KB fact lifecycle is tracked as a follow-up (the extractor only yields facts).
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

The Calendar connector is the second concrete connector backend (after
Photos). It syncs a CalDAV calendar collection into Mimir using the
sync-token protocol and stages parsed VEVENTs for the knowledge-graph
pipeline. It targets any RFC 5545 / RFC 6578 CalDAV server — Apple iCloud,
Nextcloud, Fastmail, and (with caveats) Google — and runs in `Polling` mode.

C3 (#197) delivers the **transport**: PROPFIND (calendar verification) +
sync-collection REPORT (incremental event fetch + sync-token cursor), OAuth
token **refresh** + app-password auth, and `icalendar` VEVENT parsing. It
stages parsed events in an internal buffer; **`extract()` returns no facts
yet**. C4 / #198 converts those events into `NormalizedFact`s (events,
locations, attendees → entities) and integrates them with the events &
reminders subsystem (#74), and adds write-back (`act`).

## Auth

Two credential kinds, mirroring [`SecretBundle`](connector-secret-store.md):

- **App password** (iCloud, Fastmail, Nextcloud) — HTTP Basic auth. The
  username lives in `config_json` (non-secret); the password lives in the
  `SecretStore` under the connector slug as `SecretBundle::AppPassword`.
- **OAuth 2.0** (Google Calendar) — bearer token. The access/refresh tokens
  live in the `SecretStore` as `SecretBundle::OAuth`; the non-secret client
  config (`token_endpoint`, `client_id`, optional `client_secret`/`scopes`)
  lives in `config_json`. The connector **refreshes** an expired access token
  (within a 60 s skew) before every sync/authenticate/health call and persists
  the refreshed bundle back to the store. An **unknown** expiry
  (`expires_at: None`) does *not* force a refresh on every cycle — the token
  is reused and refreshed only once it is actually expired (avoiding triple
  POSTs to the token endpoint and rate-limit risk). When a refresh response
  omits `refresh_token` (RFC 6749 §6 allows this), the connector **retains**
  the prior refresh token, so OAuth does not break after the first refresh.

The token-endpoint error path reports only the HTTP status and the parsed
`error`/`error_description` fields — the raw response body is never surfaced
to `ConnectorError` strings (which the supervisor persists to `last_error`
 and logs) because provider error payloads can echo the `client_secret` or
`refresh_token`. Auth-method/secret-kind mismatch errors use the auth-kind
discriminant only, never a `Debug` of the OAuth config.

The interactive PKCE login that *obtains* the first OAuth token is
**A4 / `#205`**, out of scope here. `#197` only consumes + refreshes a stored token.

## Sync protocol

One round trip per cycle (paged when truncated), via the
[`CalDavClient`](#caldavclient):

1. `sync-collection` REPORT (RFC 6578) on the configured `calendar_url`. The
   request body carries the required `<d:sync-level>1</d:sync-level>` element
   (RFC 6578 §6.3), the `<d:sync-token>` (omitted for a full sync), and
   `<d:prop>` requesting `<d:getetag/>` + `<cal:calendar-data/>` inline so
   changed VEVENTs and a new `sync-token` arrive together (no follow-up
   multiget).
2. Omitting `<sync-token>` performs a **full** sync and yields the initial
   token; including it performs an **incremental** sync (no re-fetch).
3. The returned `sync-token` is the connector's incremental cursor: kept
   in memory for the next in-process cycle and returned in `SyncOutcome` for
   the supervisor to persist via `KnowledgeGraph::update_sync_cursor`.
4. Each changed resource's `calendar-data` is parsed with `icalendar` into a
   `RawCalDavEvent` and staged. A `<response>` with no `calendar-data` is a
   tombstone **only** when its `<status>` is an explicit `404`/`410`; other
   statuses (`403` permission denied, `423` locked, `507` truncated, …) are
   logged and skipped so a transient server error never purges a live event
   (C4 / #198 owns fact lifecycle for deletions).
5. A truncated result set (RFC 6578 §6.5) is signalled with HTTP `507`
   (Insufficient Storage) carrying a partial multistatus body plus an
   advancing `sync-token`. The connector pages with the new token — re-issuing
   `sync-collection` and accumulating the partial changes — until a
   non-truncated response completes the sync.

`SyncOptions::full` ignores the persisted cursor (re-fetch everything).

The `roxmltree` XML helper concatenates *all* direct text/CDATA children of a
leaf element (not just the first), so `calendar-data`/`summary`/`calendar-name`
text split across multiple segments is not silently truncated.

## `CalDavClient`

`mimir_connectors::calendar::caldav::CalDavClient` wraps the workspace's
`reqwest` 0.13 client and exposes:

- `sync_collection(url, sync_token)` → `SyncCollectionResult { new_sync_token, changed, deleted }`.
- `is_calendar(url)` → PROPFIND Depth 0 `resourcetype`; used by `health` and
  `authenticate` to verify the URL is a CalDAV calendar collection.
- `put_event(href, ical, etag)` → CalDAV `PUT` (RFC 4791 §5.5) for create
  (`etag = None` sends `If-None-Match: *`) or update (`etag = Some(t)` sends
  `If-Match: t`); returns the new `ETag`.
- `delete_event(href, etag)` → CalDAV `DELETE` (RFC 4791 §5.6); idempotent on
  `404`, `If-Match: t` when an etag is known.

WebDAV XML is parsed with `roxmltree` (a read-only DOM parser) matching element
**local** names, so the varied namespace prefixes servers use (`D:`/`d:`/
`cal:`/`C:`) are tolerated. iCalendar payloads are parsed with `icalendar`'s
low-level parser (`icalendar::parser::read_calendar`) — the high-level
`icalendar::Calendar` is builder-oriented with no parse-from-str path in
0.17.x.

## Event → KB fact extraction (C4 / #198)

`CalendarConnector::extract` drains the staged VEVENTs into a cluster of `NormalizedFact`s, which the supervisor hands to the shared `normalize_and_insert` pipeline (entity resolution via F5, connector confidence, sensitivity gate, corroboration/supersession inherited). Per VEVENT it emits one primary fact, optionally one location fact, and one fact per attendee:

- **`user has_event <event>`** — the primary appointment. The subject is the canonical user identity (the `config.toml` `[identity] name`, injected via `ConnectorContext::user_identity` / `ConnectorSupervisor::with_user_identity`) so the event surfaces in the user's "Upcoming" memory section, which is scoped to the user entity. The object is an `Event` entity named by the `SUMMARY` (falling back to the `UID`). It carries the temporal bounds (`DTSTART`/`DTEND`), the recurrence (mapped from `RRULE` `FREQ`), and an `EventType::Appointment` hint so the events-subsystem (#74) overlay is typed correctly rather than defaulting to `Reminder`. When no user identity is configured the primary fact is skipped (the event is still captured via its location/attendee facts, but it will not appear in Upcoming).
- **`<event> located_in <place>`** — the `LOCATION` resolves to a `Place` entity via F5. The venue is a property of the event, not the user's location history, so this fact carries no `entity_locations` overlay (a calendar full of meetings would otherwise bloat `Visited` rows). It also carries no temporal bounds (`valid_from`/`valid_until`), so it spawns no events-subsystem overlay — only the primary `has_event` fact drives one.
- **`<attendee> attending <event>`** — each `ATTENDEE` resolves to a `Person` entity via F5 (the `CN` parameter, else the `mailto:` value). Like the location fact it carries no temporal bounds and spawns no events-subsystem overlay.

Dates are parsed to UTC at staging time: `DTSTART`/`DTEND` may be UTC (`…Z`), floating local, date-only, or `TZID`-qualified; the latter is resolved via `chrono-tz` (an unknown zone falls back to the naive value read as UTC so a bad `TZID` never drops the event). Only `RRULE` `FREQ` maps to the KB's coarse `RecurrenceType` (Daily/Weekly/Monthly/Yearly); `COUNT`, `UNTIL`, `INTERVAL`, and `BYxxx` are out of scope (the events-subsystem is a per-`FREQ` next-occurrence model, not a full RFC 5545 expander).

The `event_type: Option<EventType>` hint added to `NormalizedFact` is the mechanism: connectors that know the event kind supply it; chat leaves it `None` so the existing `Task`/`Reminder` derivation is unchanged.

Server-side deletions (tombstones) are logged during `sync` but not yet propagated to the KB — surfacing a deletion needs a way for the connector to report removals (`extract` only yields facts), so trashing the corresponding facts is tracked as a follow-up.

## Write-back (C4 / #198)

`CalendarConnector::act` is the only connector with write support. Three action kinds, each authenticated via the same credential path as `sync` (OAuth refresh included):

- `create_event` — builds a VEVENT from the payload (the `icalendar` builder), generates a `UID` (unless supplied), and `PUT`s it to `<calendar>/<uid>.ics` with `If-None-Match: *`.
- `update_event` — requires the target `href` (and optional `etag`); `PUT`s with `If-Match: <etag>`.
- `delete_event` — requires the target `href` (and optional `etag`); `DELETE`s it (idempotent on 404).

The `start`/`end` payload fields are RFC-3339 datetimes. `attendees` are bare addresses (an optional `mailto:` prefix is normalised). The returned `ActionResult::native_id` is the resource href; `message` carries the new `ETag` when the server supplies one.
Every action `href` is validated against the configured `calendar_url` before the request is issued: the scheme, host, and port must match the configured origin and the path must lie under the calendar collection, so a caller-supplied URL cannot redirect the connector's stored credentials to another host or an unrelated resource on the same host.

## Secret-store injection

The Calendar connector is the first backend that needs credentials, so it
extends the framework's `ConnectorContext` with a `secret_store:
Option<Arc<dyn SecretStore>>` field and adds
`ConnectorSupervisor::with_secret_store(store)`. The factory clones the store
out of the context at construction; the connector loads its bundle by slug
(the `__slug` the supervisor injects into `config_json`). This is a breaking
change to an internal construction-context API, which the project's
breaking-changes policy explicitly allows.

C4 (#198) extends the same context with the canonical **user identity name**
(`ConnectorContext::user_identity` / `ConnectorSupervisor::with_user_identity`)
so the connector authors `user has_event <event>` against the same entity the
daemon resolves as `user_entity_id` (and the event surfaces in the user's
"Upcoming" section). The Photos connector predates this and carries its own
disconnected `owner_name` config field — aligning it with the shared identity
is tracked as a follow-up.

## Dependencies

All optional, gated by the `calendar` feature:

| Crate | Version | Role |
|-------|---------|------|
| `icalendar` | 0.17.x | Strongly-typed RFC 5545 iCalendar parser (default `parser` feature). **Resolves to 0.17.6 under the workspace MSRV (1.85); 0.17.12 requires Rust 1.88** — see the follow-up issue tracking the deps-ledger / MSRV reconciliation. |
| `roxmltree` | 0.21 | Pure-Rust read-only DOM XML parser for WebDAV multistatus responses. |
| `reqwest` | 0.13 (in tree) | HTTP (CalDAV + the `OAuthHttpClient` adapter's transport). |
| `oauth2` | 5.0.0 (`default-features = false`) | Vetted OAuth 2.0 protocol code (refresh grant today, PKCE in A4 / #205); talks HTTP through the `OAuthHttpClient` adapter over the workspace reqwest 0.13 client (issue #240). Gated by the `oauth` feature. |
| `chrono-tz` | 0.10 | IANA timezone database for `chrono`; resolves `TZID`-qualified `DTSTART`/`DTEND` to UTC. New in C4. |
| `uuid` | 1 (in tree) | `v4` UID generation for new write-back events. New in C4. |

OAuth token refresh runs on the vetted `oauth2` crate (issue #240): `oauth2`
5.0.0 is pulled with `default-features = false` so its optional reqwest 0.12
dependency never enters the tree, and a custom `OAuthHttpClient` adapter
implements the crate's `AsyncHttpClient` trait over the workspace's single
reqwest 0.13 client (see [OAuth client](oauth-client.md)). The adapter's client
never follows redirects (a credential POST cannot be bounced to another host),
the HTTPS/loopback endpoint gate is preserved, provider response errors surface
only the parsed `error`/`error_description` fields (never the raw response
body), and network failures include the underlying reqwest error detail.

## Config

Stored as `config_json` on the `connectors` row (with `__slug` / `__cursor`
injected by the supervisor):

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
    "token_endpoint": "https://oauth2.googleapis.com/token",
    "client_id": "mimir-client",
    "scopes": ["https://www.googleapis.com/auth/calendar.readonly"]
  }
}
```

## Tests

- Unit (`src/calendar/caldav/`): sync-collection full/incremental parse,
  401 handling, PROPFIND resourcetype calendar detection, `icalendar` field
  extraction + recurrence, invalid-payload resilience — all against a
  `wiremock` mock CalDAV server.
- Integration (`tests/calendar_sync_tests.rs`): app-password sync, incremental
  sync-token, `full`-sync cursor reset, OAuth refresh-on-expiry + bundle
  persistence, health (online / not-configured / auth-expired), factory
  construction + config round-trip, and a full `ConnectorSupervisor` round-trip
  asserting the cursor is persisted on the connector row.

No `unsafe`; honours the workspace `#![deny(unsafe_code)]` guarantee.
