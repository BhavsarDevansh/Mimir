# Email Connector

> **Phase:** 3 — Connectors
> **Status:** Library done — C5 transport (#199) + C6 structured extraction (#200, calendar invites). LLM extraction for flights/bookings/prose is C7 (#201); deterministic `schema.org` JSON-LD extraction is #249; daemon wiring and the `mimir connector …` CLI come in later Phase 3 issues (A1–A3).

## What it is

The Email connector reads your mailbox (Gmail, Outlook/Hotmail, iCloud Mail — any IMAP server) into Mimir and turns your mail into knowledge-graph facts. It speaks IMAP — the open mail protocol your provider already supports, so it works with any compliant server, no vendor lock-in.

It is a background sync worker that runs in two modes automatically:

- **Push (IMAP IDLE)** — when your server supports it (Gmail, Outlook, and iCloud all do), the connector stays connected and is notified the instant a new email arrives, then fetches just that message. Near-real-time, and very cheap.
- **Polling (fallback)** — for servers without IDLE, the connector checks for new mail every few minutes. The connector detects which mode to use on its own.

## How it works

- You point it at your IMAP server and give it either an **app-specific password** or an **OAuth** token (Google/Microsoft). The secret lives in Mimir's permission-checked secret store (`0600`); the username and server config live in the connector config.
- Each sync fetches only what's new. Mimir tracks the last message it saw by **UID** (a stable, server-assigned message id), so it never re-downloads the same email. Your progress is saved across restarts.
- If your mailbox is ever recreated (a rare event Mimir detects via a server value called `UIDVALIDITY`), the connector notices and does one full re-fetch — no silent gaps or duplicates.
- Each message's raw contents (headers, body) are held in an in-memory buffer ready for the knowledge graph.

> **What's done so far:** The transport (#199) logs in, watches for new mail, and fetches messages. C6 (#200) then turns the structured subset — **calendar invites** (emails with a `text/calendar` attachment) — into knowledge-graph facts: a dentist, airline, or colleague sending a real invite creates an appointment in your "Upcoming" section, with the time, location, and attendees. Free-text confirmations, flight boarding passes, bookings, and bank statements (emails with no calendar invite) are read by the LLM layer in C7 (#201); transactional emails that embed machine-readable `schema.org` data will be a future deterministic layer (#249).

### The email is the evidence, not the fact

Mimir does **not** record "I received an email from the dentist" — that is just evidence. It records the real-world thing the email conveys (an appointment, with a date and a place). It also does **not** turn every sender or recipient into a contact: marketing and spam emails produce no facts at all, so your knowledge graph isn't filled with junk. The email's message id is kept as the provenance so you can always trace a fact back to the email it came from.

## Authentication

- **App password** — best for most providers. Generate an app-specific password in your provider's security settings (Gmail calls them "app passwords"); Mimir uses standard IMAP `LOGIN`. Your username is in the connector config; the password is stored securely.
- **OAuth (Google / Microsoft)** — the connector stores your access + refresh token and refreshes the access token automatically before it expires, so you stay connected without re-authorising. The first token is obtained via an interactive sign-in flow that arrives in a later issue (#206).

## Privacy

- Mimir only **reads** mail — it never sends, deletes, or marks messages. It uses `BODY.PEEK[]` so your unread mail stays unread.
- All data is fetched and stored locally; no cloud intermediary.
- "Forget everything from Gmail" is a library-level capability (the connector's `forget()` wipes its cursor, buffer, and stored secret) — it is not yet exposed through a `mimir connector …` command until the daemon wiring (A1–A3 / #202–#204) lands.

## Config example

```json
{
  "host": "imap.gmail.com",
  "port": 993,
  "mailbox": "INBOX",
  "auth": { "kind": "app_password", "username": "you@gmail.com" },
  "mode": "auto"
}
```

`mode` can be `"auto"` (default — IDLE if supported, else polling), `"idle"`, or `"poll"`. V1 syncs a single mailbox (`INBOX` by default, configurable).
