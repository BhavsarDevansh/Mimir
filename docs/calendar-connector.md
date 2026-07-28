# Calendar Connector (CalDAV) — `mimir-connectors::calendar`

> **Phase:** 3 — Connectors (C3 / issue #197)
> **Feature flag:** `calendar` (default). Framework + mock stay built without it.
> **Status:** Implemented (library only). Event → KB fact extraction, the
> events-subsystem (#74) integration, and write-back (`act`) are C4 / #198; the
> daemon `AppState` wiring + `mimir connector …` CLI land in A1–A3 (#202–#204);
> the interactive OAuth PKCE login is A4 / #206.
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
  the prior refresh token so OAuth does not break after the first refresh.

The token-endpoint error path reports only the HTTP status and the parsed
`error`/`error_description` fields — the raw response body is never surfaced
to `ConnectorError` strings (which the supervisor persists to `last_error`
and logs), because provider error payloads can echo the `client_secret` or
`refresh_token`. Auth-method/secret-kind mismatch errors use the auth-kind
discriminant only, never a `Debug` of the OAuth config.

The interactive PKCE login that *obtains* the first OAuth token is
**A4 / `#206`**, out of scope here. `#197` only consumes + refreshes a stored token.

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

WebDAV XML is parsed with `roxmltree` (a read-only DOM parser) matching element
**local** names, so the varied namespace prefixes servers use (`D:`/`d:`/
`cal:`/`C:`) are tolerated. iCalendar payloads are parsed with `icalendar`'s
low-level parser (`icalendar::parser::read_calendar`) — the high-level
`icalendar::Calendar` is builder-oriented with no parse-from-str path in
0.17.x.

## Secret-store injection

The Calendar connector is the first backend that needs credentials, so it
extends the framework's `ConnectorContext` with a `secret_store:
Option<Arc<dyn SecretStore>>` field and adds
`ConnectorSupervisor::with_secret_store(store)`. The factory clones the store
out of the context at construction; the connector loads its bundle by slug
(the `__slug` the supervisor injects into `config_json`). This is a breaking
change to an internal construction-context API, which the project's
breaking-changes policy explicitly allows.

## Dependencies

All optional, gated by the `calendar` feature:

| Crate | Version | Role |
|-------|---------|------|
| `icalendar` | 0.17.x | Strongly-typed RFC 5545 iCalendar parser (default `parser` feature). **Resolves to 0.17.6 under the workspace MSRV (1.85); 0.17.12 requires Rust 1.88** — see the follow-up issue tracking the deps-ledger / MSRV reconciliation. |
| `roxmltree` | 0.21 | Pure-Rust read-only DOM XML parser for WebDAV multistatus responses. |
| `reqwest` | 0.13 (in tree) | HTTP; the `form` feature was added for the OAuth refresh token POST. |

The `oauth2` crate is **deliberately not** pulled in: `oauth2` 5.0.0 depends on
`reqwest` 0.12, which would duplicate the workspace's reqwest 0.13 HTTP/TLS
stack, and #197 only needs the refresh grant (a single form-encoded POST). It
is deferred to A4 / #206, where the PKCE authorization-code flow justifies it.
See the follow-up issue tracking the deps-ledger / reqwest-0.13 reconciliation.

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

- Unit (`src/calendar/caldav.rs`): sync-collection full/incremental parse,
  401 handling, PROPFIND resourcetype calendar detection, `icalendar` field
  extraction + recurrence, invalid-payload resilience — all against a
  `wiremock` mock CalDAV server.
- Integration (`tests/calendar_connector.rs`): app-password sync, incremental
  sync-token, `full`-sync cursor reset, OAuth refresh-on-expiry + bundle
  persistence, health (online / not-configured / auth-expired), factory
  construction + config round-trip, and a full `ConnectorSupervisor` round-trip
  asserting the cursor is persisted on the connector row.

No `unsafe`; honours the workspace `#![deny(unsafe_code)]` guarantee.
