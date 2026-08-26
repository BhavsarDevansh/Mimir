# Calendar Connector

> **Phase:** 3 — Connectors
>
> **Status:** Implemented (library + daemon/CLI) — C3 (#197) transport + read/sync, C4 (#198) event → knowledge-graph extraction, events-subsystem integration, write-back, and the interactive OAuth PKCE login (A4 / #205). Server-side deletions (tombstones) are propagated to the KB fact lifecycle (#247). Issue #474 adds the Microsoft Graph backend for Outlook / Office 365 calendars (OAuth-only, read-only, same fact shapes).

## What it is

The Calendar connector reads your CalDAV calendar (Apple iCloud, Nextcloud, Fastmail, and similar) into Mimir, **staging** your events so the knowledge graph can answer **what events you have, where, and when** — C4 (#198) turns those staged events into facts (with locations and attendees resolved to entities) and can write events back to the calendar. It speaks CalDAV — the open calendar protocol your calendar server already supports — so it works with any compliant server, no vendor lock-in.

It is a background sync worker: it periodically pulls new/changed events using CalDAV's **sync-token** protocol (only the deltas since the last sync, not the whole calendar every time) and stages them for the knowledge graph.

## How it works

- You point it at a calendar URL and give it either an **app-specific password** (iCloud/Fastmail/Nextcloud) or an **OAuth** token (Google). The secret lives in Mimir's permission-checked secret store (`0600`); the current backend stores credentials in plaintext at rest, never in plain config.
- Each sync issues one CalDAV `sync-collection` request. The first time it fetches everything and gets a **sync-token**; every later sync sends that token back and receives only what changed (new/updated/deleted events) plus a fresh token. So syncs are cheap and incremental.
- Each event's iCalendar payload (UID, summary, start/end, location, recurrence rule) is parsed and held in an in-memory buffer ready for the knowledge graph.
- When the server reports an event as **deleted** (a `sync-collection` tombstone), the connector stages the deletion's href and the supervisor trashes that event's facts in the knowledge graph (recoverable from trash for 30 days), so a calendar event you cancel or delete in another client stops surfacing in **Upcoming** instead of living on as a phantom.
- The connector keeps the sync-token as its progress marker: across restarts it normally resumes from where it left off; a requested full sync or an invalidated cursor can require a complete refetch.
- Syncs are **failure-safe**: the progress marker only advances after a sync cycle fully succeeded (fetch → extract → insert → save). If a cycle fails part-way (a temporary extraction problem, for example), the next cycle re-syncs from the last saved marker and re-processes the events that cycle had fetched — nothing is silently skipped, so you never lose an event from your knowledge graph to a transient glitch ([#314](https://github.com/BhavsarDevansh/Mimir/issues/314)).

> **C3 vs C4:** #197 did the *transport* — fetching and parsing your events. #198 turns those events into knowledge-graph facts (with locations and attendees resolved to entities, recurring events advanced by the events & reminders subsystem) and adds write-back to create/update/delete remote events.

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

Deleting an event on the server (in another client) also removes the corresponding KB facts automatically: the connector stages the sync-collection tombstone's href and the supervisor trashes the matching facts (recoverable from trash for 30 days) so the event stops surfacing in Upcoming (#247). **Upgrade note (0.103.0):** facts authored before 0.103.0 carry the event `UID` as their raw reference, so href-based tombstones cannot match them and a pre-upgrade deletion would leave the event active in the knowledge graph and Upcoming; if you upgraded from an earlier version, remove each calendar instance's pre-upgrade facts (`mimir connector forget <slug>`, recoverable from trash for 30 days) and trigger a full re-sync so the events are re-authored with href references.

## Authentication

- **App password** — best for iCloud/Fastmail/Nextcloud. Generate an app-specific password in your provider's settings; Mimir uses HTTP Basic auth. Your username is in the connector config; the password is stored securely.
- **OAuth (Google)** — the connector stores your access + refresh token and **refreshes** the access token automatically before each sync when it expires. The initial sign-in is the interactive OAuth PKCE dance (A4 / #205): `mimir connector add calendar … auth.kind=oauth …` opens the provider's authorize URL in your browser, receives the redirect on a loopback listener, and stores the exchanged token bundle.

Mimir can also write back to your calendar: creating, updating, or deleting remote events via CalDAV `PUT`/`DELETE` (added in C4 / #198). This is the only connector with write support.

## Microsoft Graph (Outlook / Office 365) — issue #474

Outlook.com and Office 365 calendars cannot be read over CalDAV (Microsoft exposes no public CalDAV endpoint), so Mimir's calendar connector has a second backend for them: **Microsoft Graph**. You pick `Calendar (graph)` in the wizard (or `mimir connector add calendar --backend graph`), and Mimir syncs your default calendar through Microsoft's Graph API using **delta sync** — the first sync imports your events, and every later sync fetches only what changed since the last one, so syncs stay cheap.

- **Authentication** — OAuth 2.0 only. The wizard asks which Microsoft account type your app registration targets (personal Outlook.com/Hotmail, work or school, either, or a single tenant) and pre-fills the matching Microsoft login endpoints — it never hardcodes the `/common/` endpoint that silently breaks personal-only or org-only registrations. You bring your own app registration from the Microsoft Entra admin center (Mimir has no public client ID): its "Supported account types" must match the audience you pick, the loopback redirect URI `http://localhost/callback` must be registered, and the app needs the `Calendars.Read` delegated permission. The first login is the same interactive browser flow as the other OAuth connectors; Mimir then refreshes the access token automatically.
- **What syncs** — events from your default calendar become the same knowledge-graph facts as CalDAV events: `you have_event <event>` (with start/end and recurrence, so future and recurring events surface in **Upcoming**), `<event> located_in <place>`, and `<attendee> attending <event>`. Deleted events are removed from the knowledge graph automatically (recoverable from trash for 30 days).
- **Read-only** — like every connector today, the Graph backend only imports; write-back (creating/editing events from Mimir) is not implemented for it.

## Use cases

- "What do I have on Thursday?" — your calendar events become queryable knowledge, cross-referenced with everything else Mimir knows.
- Recurring events (birthdays, standups) sync once and advance automatically via the events & reminders subsystem.
- Travel: a calendar event "Trip to Rome" can corroborate a flight email and photos taken in Rome into one coherent picture.

## Known limitations (V1)

- Google's CalDAV sync-token support is non-standard; the generic client works against fully RFC 6578-compliant servers (iCloud, Nextcloud). Google-specific handling is a follow-on.
- The interactive OAuth login (PKCE) is wired (A4 / #205); Google-specific CalDAV sync-token handling remains a follow-on.
- Event → knowledge-graph extraction, write-back, and server-side deletion propagation are live (C4 / #198 + #247); richer `RRULE` recurrence rules are a follow-up.
