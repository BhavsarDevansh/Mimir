# Calendar Connector

> **Phase:** 3 — Connectors
> **Status:** Implemented (library + daemon/CLI) — C3 (#197) transport + read/sync, C4 (#198) event → knowledge-graph extraction, events-subsystem integration, write-back, and the interactive OAuth PKCE login (A4 / #205). Server-side deletion → KB fact lifecycle is a follow-up.

## What it is

The Calendar connector reads your CalDAV calendar (Apple iCloud, Nextcloud,
Fastmail, and similar) into Mimir, **staging** your events so the knowledge
graph can answer **what events you have, where, and when** — C4 (#198) turns
those staged events into facts (with locations and attendees resolved to
entities) and can write events back to the calendar. It speaks CalDAV — the open calendar protocol your calendar server
already supports — so it works with any compliant server, no vendor lock-in.

It is a background sync worker: it periodically pulls new/changed events
using CalDAV's **sync-token** protocol (only the deltas since the last sync,
not the whole calendar every time) and stages them for the knowledge graph.

## How it works

- You point it at a calendar URL and give it either an **app-specific
  password** (iCloud/Fastmail/Nextcloud) or an **OAuth** token (Google). The
  secret lives in Mimir's permission-checked secret store (`0600`); the
  current backend stores credentials in plaintext at rest, never in plain
  config.
- Each sync issues one CalDAV `sync-collection` request. The first time it
  fetches everything and gets a **sync-token**; every later sync sends that
  token back and receives only what changed (new/updated/deleted events) plus a
  fresh token. So syncs are cheap and incremental.
- Each event's iCalendar payload (UID, summary, start/end, location,
  recurrence rule) is parsed and held in an in-memory buffer ready for the
  knowledge graph.
- The connector keeps the sync-token as its progress marker: across restarts
  it normally resumes from where it left off; a requested full sync or an
  invalidated cursor can require a complete refetch.

> **C3 vs C4:** #197 did the *transport* — fetching and parsing your
> events. #198 turns those events into knowledge-graph facts (with locations
> and attendees resolved to entities, recurring events advanced by the
> events & reminders subsystem) and adds write-back to create/update/delete
> remote events.

## How events become knowledge (C4 / #198)

When the connector extracts your staged events, each one becomes a small cluster of facts in the knowledge graph:

- **The event itself** — `you have_event <event>` (e.g. "Devansh has_event Trip to Rome"). The event is an entity named by its title; the fact carries the start/end times and recurrence. Future-dated and recurring events then surface in your **Upcoming** section, so "what do I have this week?" works across calendar and conversational events alike.
- **The location** — `<event> located_in <place>`; the venue resolves to a `Place` entity, so events are searchable by where they happen.
- **The attendees** — `<attendee> attending <event>`; each attendee resolves to a `Person` entity, growing your contact graph from your calendar.

The connector authors these facts as your canonical identity (the `[identity] name` in your config), so calendar events line up with the same "you" the rest of Mimir uses. Recurring events (a weekly standup, a yearly birthday) advance automatically via the events & reminders subsystem — only the `RRULE` frequency maps (daily/weekly/monthly/yearly); richer recurrence rules are a future enhancement. Dates are normalised to UTC, including time-zone-qualified ones.

## Write-back (C4 / #198)

The Calendar connector is the only connector that can write back to its source. Three actions are supported (wired to the daemon/CLI in A1–A3):

- **create_event** — builds a new VEVENT and `PUT`s it to the calendar (`If-None-Match: *` so a stray overwrite fails instead of clobbering).
- **update_event** — `PUT`s with `If-Match: <etag>` to a known event href.
- **delete_event** — `DELETE`s an event (idempotent — a 404 is treated as success).

A delete on the server does not yet remove the corresponding KB fact automatically; that lifecycle is a follow-up.

## Authentication

- **App password** — best for iCloud/Fastmail/Nextcloud. Generate an
  app-specific password in your provider's settings; Mimir uses HTTP Basic
  auth. Your username is in the connector config; the password is stored
  securely.
- **OAuth (Google)** — the connector stores your access + refresh token and
  **refreshes** the access token automatically before each sync when it
  expires. The initial sign-in is the interactive OAuth PKCE dance (A4 /
  #205): `mimir connector add calendar … auth.kind=oauth …` opens the
  provider's authorize URL in your browser, receives the redirect on a
  loopback listener, and stores the exchanged token bundle.

Mimir can also write back to your calendar: creating, updating, or deleting
remote events via CalDAV `PUT`/`DELETE` (added in C4 / #198). This is the
only connector with write support.

## Use cases

- "What do I have on Thursday?" — your calendar events become queryable
  knowledge, cross-referenced with everything else Mimir knows (once C4 turns
  staged events into facts).
- Recurring events (birthdays, standups) sync once and advance automatically
  via the events & reminders subsystem (once C4 lands).
- Travel: a calendar event "Trip to Rome" can corroborate a flight email and
  photos taken in Rome into one coherent picture.

## Known limitations (V1)

- Google's CalDAV sync-token support is non-standard; the generic client works
  against fully RFC 6578-compliant servers (iCloud, Nextcloud). Google-specific
  handling is a follow-on.
- The interactive OAuth login (PKCE) is wired (A4 / #205); Google-specific
  CalDAV sync-token handling remains a follow-on.
- Event → knowledge-graph extraction and write-back are live (C4 / #198);
  richer `RRULE` recurrence rules and server-side deletion → KB fact lifecycle
  are follow-ups.
