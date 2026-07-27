# Calendar Connector

> **Phase:** 3 — Connectors
> **Status:** Library done — C3 (#197). Event → knowledge-graph extraction,
> event reminders, and write-back come in C4 (#198). Daemon wiring and the
> `mimir connector …` CLI come in later Phase 3 issues (A1–A3).

## What it is

The Calendar connector reads your CalDAV calendar (Apple iCloud, Nextcloud,
Fastmail, and similar) into Mimir so your knowledge graph knows **what events
you have, where, and when**. It speaks CalDAV — the open calendar protocol
your calendar server already supports — so it works with any compliant server,
no vendor lock-in.

It is a background sync worker: it periodically pulls new/changed events
using CalDAV's **sync-token** protocol (only the deltas since the last sync,
not the whole calendar every time) and stages them for the knowledge graph.

## How it works

- You point it at a calendar URL and give it either an **app-specific
  password** (iCloud/Fastmail/Nextcloud) or an **OAuth** token (Google). The
  secret lives in Mimir's encrypted-permission secret store (`0600`), never in
  plain config.
- Each sync issues one CalDAV `sync-collection` request. The first time it
  fetches everything and gets a **sync-token**; every later sync sends that
  token back and receives only what changed (new/updated/deleted events) plus a
  fresh token. So syncs are cheap and incremental.
- Each event's iCalendar payload (UID, summary, start/end, location,
  recurrence rule) is parsed and held in an in-memory buffer ready for the
  knowledge graph.
- The connector keeps the sync-token as its progress marker: across restarts
  it resumes from where it left off, never re-fetching the whole calendar.

> **C3 vs C4:** This first cut (#197) does the *transport* — it fetches and
> parses your events. Turning those events into knowledge-graph facts (with
> locations and attendees resolved to entities, recurring events advanced by
> the events & reminders subsystem, and write-back to create/update/delete
> remote events) is C4 (#198).

## Authentication

- **App password** — best for iCloud/Fastmail/Nextcloud. Generate an
  app-specific password in your provider's settings; Mimir uses HTTP Basic
  auth. Your username is in the connector config; the password is stored
  securely.
- **OAuth (Google)** — the connector stores your access + refresh token and
  **refreshes** the access token automatically before each sync when it
  expires. The initial sign-in (the OAuth PKCE dance) is added in a later
  issue (A4 / #206); for now you supply the first token.

Mimir is read-only against your calendar in this release (no events are
created, modified, or deleted). Write-back lands in C4 (#198).

## Use cases

- "What do I have on Thursday?" — your calendar events become queryable
  knowledge, cross-referenced with everything else Mimir knows.
- Recurring events (birthdays, standups) sync once and advance automatically
  via the events & reminders subsystem (once C4 lands).
- Travel: a calendar event "Trip to Rome" can corroborate a flight email and
  photos taken in Rome into one coherent picture.

## Known limitations (V1)

- Google's CalDAV sync-token support is non-standard; the generic client works
  against fully RFC 6578-compliant servers (iCloud, Nextcloud). Google-specific
  handling is a follow-on.
- The interactive OAuth login (PKCE) is not yet wired — supply the first token
  manually until A4 (#206).
- No event → knowledge-graph extraction yet (#197 only fetches/parses); that is
  C4 (#198).
