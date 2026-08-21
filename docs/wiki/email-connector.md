# Email Connector

> **Phase:** 3 — Connectors
>
> **Status:** Implemented (library + daemon/CLI) — C5 transport (#199) + C6 structured extraction (#200, calendar invites) + #249 (schema.org JSON-LD deterministic extraction) + C7 LLM extraction (#201, unstructured prose) + the interactive OAuth PKCE login (A4 / #205).

## What it is

The Email connector reads your mailbox (Gmail, Outlook/Hotmail, iCloud Mail — any IMAP server) into Mimir and turns your mail into knowledge-graph facts. It speaks IMAP — the open mail protocol your provider already supports, so it works with any compliant server, no vendor lock-in.

It is a background sync worker that runs in two modes automatically:

- **Push (IMAP IDLE)** — when your server supports it (Gmail, Outlook, and iCloud all do), the connector stays connected and is notified the instant a new email arrives, then fetches just that message. Near-real-time, and very cheap.
- **Polling (fallback)** — for servers without IDLE, the connector checks for new mail every few minutes. The connector detects which mode to use on its own.

## How it works

- You point it at your IMAP server and give it either an **app-specific password** or an **OAuth** token (Google/Microsoft). The secret lives in Mimir's permission-checked secret store (`0600`); the username and server config live in the connector config.
- Each sync fetches only what's new. Mimir tracks the last message it saw by **UID** (a stable, server-assigned message id), so it never re-downloads the same email. Your progress is saved across restarts.
- Sync is **failure-safe**: Mimir only records how far it got after a sync fully succeeds (fetch, extract, and knowledge-graph insert all complete). If a sync fails part-way, the next run re-fetches the same window instead of skipping it, so no email is silently lost. This also applies in push/IDLE mode: after a failed sync the next run re-fetches immediately instead of waiting for the next new email.
- If your mailbox is ever recreated (a rare event Mimir detects via a server value called `UIDVALIDITY`), the connector notices and does one full re-fetch — no silent gaps or duplicates.
- Each message's raw contents (headers, body) are held in an in-memory buffer ready for the knowledge graph.


### The email is the evidence, not the fact

Mimir does **not** record "I received an email from the dentist" — that is just evidence. It records the real-world thing the email conveys (an appointment, with a date and a place), surfacing it in your "Upcoming" section only when a canonical user identity is configured (`ConnectorContext::user_identity`). It also does **not** turn every sender or recipient into a contact: a message without a supported iMIP `REQUEST`/`REPLY` part produces no facts at all, so your knowledge graph isn't filled with junk — a supported invitation produces facts regardless of who sent it. Invitation facts are provenanced by the event's own stable id (the iCalendar `UID`), so when the organiser sends a **cancellation** (`CANCEL`) Mimir recognises the same event, removes its facts from the knowledge graph (recoverable from trash for 30 days), and it stops appearing in "Upcoming" — no phantom appointments. A malformed invite without a `UID` falls back to the email's UIDVALIDITY-qualified IMAP UID as provenance, and a `CANCEL` without a `UID` is skipped because it cannot be mapped to prior facts. Transactional facts (flights, bookings, orders) keep the email's UIDVALIDITY-qualified IMAP UID as provenance so you can always trace them back to the email they came from. **Upgrade note (0.110.0):** invitation facts authored before 0.110.0 carry the email's UIDVALIDITY-qualified IMAP UID as their provenance, so a `CANCEL` cannot match them and a pre-upgrade cancellation would leave the event active in the knowledge graph and "Upcoming"; if you upgraded from an earlier version, remove each email instance's pre-upgrade facts (`mimir connector forget <slug>`, recoverable from trash for 30 days), re-add and re-authenticate the Email connector (forget removes the connector row and credentials), and trigger a full re-sync so invites are re-authored with VEVENT-UID references.

### Transactional email with schema.org JSON-LD (#249)

Many transactional emails — flight confirmations, hotel bookings, e-commerce orders, delivery tracking, event tickets — embed machine-readable `schema.org` JSON-LD in `<script type="application/ld+json">` tags within their HTML body. Mimir scans for these blocks and extracts deterministic, typed facts with no LLM involved:

- **Flights** (`FlightReservation`): "I have a flight British Airways 123" with departure and arrival times, origin and destination airports, and the airline.
- **Hotel stays** (`LodgingReservation`): "I have a booking at Grand Hotel" with check-in and check-out dates and the hotel address.
- **Events** (`EventReservation`): "I have an event Symphony Concert" with start/end times and the venue.
- **Orders** (`Order`): "I have an order ORD-99" with the order date and the merchant.
- **Deliveries** (`ParcelDelivery`): "I have a delivery TRK123" with the expected arrival window, carrier, and delivery address.
- **Tickets** (`Ticket`): "I have a ticket TKT-7" with the issue date and issuer.
- **Multi-leg flights** (`ReservationPackage`): each leg is extracted as its own flight fact cluster.

Unrecognised JSON-LD types are skipped (never guessed). The primary "I have a…" fact is only emitted when a user identity is configured; the secondary facts (airports, airlines, venues, carriers) are always captured. All facts carry the email's IMAP UID as provenance and are deduplicated automatically if the same confirmation email arrives twice.

### Unstructured prose with the LLM layer (C7 / #201)

Not every email is machine-readable. A dentist's "see you Tuesday 3pm" with no calendar attachment, a flight confirmation written in plain prose, a bank statement, a job offer — these carry no `text/calendar` part and no `schema.org` JSON-LD. For those, Mimir has a third, last-resort layer that reads the body with the LLM:

- It only runs on emails the deterministic layers (iMIP invites, JSON-LD) produced **no** facts for, so a machine-readable email is never re-processed by the LLM (no duplicates, bounded cost).
- The LLM must call a strict `extract_email_facts` tool with a closed schema. Mimir **validates** every field in Rust against its typed enums before turning the output into a fact — entity types, dates, the event kind (`Appointment`/`Deadline`/`Task`/…), recurrence, and any location. An unrecognised value is dropped, never trusted.
- Obvious bulk-marketing mail (sent from pure marketing platforms like Mailchimp and HubSpot, or any mail carrying a `List-Unsubscribe` header) is skipped **before** any LLM call by a deterministic Rust filter, so marketing mail never costs an API call and never becomes junk facts. Transactional receipts and bookings routed through general-purpose ESPs (SendGrid, Mailgun, Postmark, Amazon SES) still reach the LLM. For everything else, the LLM simply returns an empty fact list when there is nothing to extract.
- The LLM call runs on Mimir's shared LLM **system queue** — at lower priority than your chat — so a connector call waiting in the queue never delays a queued chat, and a chat you send mid-extraction jumps ahead of any connector call that has not started yet. This is what the C7 acceptance criterion ("a queued chat preempts a waiting connector call") guarantees: pre-emption applies to connector calls waiting in the pool, not to a provider request already in flight.
- User-scoped facts ("I have a flight…", "I have an appointment…") are authored against your canonical identity so they resolve to you and surface in your "Upcoming" section.

### If the LLM layer fails (#262)

Sometimes the LLM cannot read an email — a provider hiccup, a network error, or a message the model refuses to process. Mimir never treats that as "nothing to extract": the `connector_item.remember` hook retries the message with backoff, while any facts the deterministic layers already found are kept. The retry is **bounded and restart-safe**:

- Each message gets a small retry budget (3 attempts by default, configurable via `llm_extraction_max_attempts`) with an increasing wait between attempts, so a stuck message cannot keep burning LLM calls forever.
- Once the budget is exhausted the message is recorded as **permanently failed with the reason** and skipped; it never consumes another LLM call. A re-fetched message in a new mailbox epoch (a `UIDVALIDITY` change) is treated as a new message and gets a fresh chance.
- The retry state — including the raw message bytes (capped at 512 KiB, and at most 32 pending messages' bytes are saved per sync: beyond the cap, messages still retry within the running daemon, but only the saved ones resume after a restart) — is saved with the connector between restarts, so a `mimir stop` or reboot resumes the retry where it left off instead of silently dropping the email.
- If any messages have permanently failed, the connector reports itself as **degraded** in its health status so the situation is visible, and "forget" clears the recorded failures along with everything else.

This is a library component today (in `mimir-connectors`); the daemon wiring that turns it on for a running `mimir` daemon lands with #202.

## Authentication

- **App password** — best for most providers. Generate an app-specific password in your provider's security settings (Gmail calls them "app passwords"); Mimir uses standard IMAP `LOGIN`. Your username is in the connector config; the password is stored securely.
- **OAuth (Google / Microsoft)** — the connector stores your access + refresh token and refreshes the access token automatically before it expires, so you stay connected without re-authorising. The first token is obtained via the interactive PKCE sign-in flow (A4 / #205): running `mimir connector add` with no arguments lists the supported `(connector_type, backend)` pairs and lets you select Gmail, then offers OAuth browser login with Google's authorization/token endpoints pre-filled (you supply your own OAuth client ID from the Google Cloud Console), launches your browser at the printed authorize URL, and stores the exchanged token bundle. The flag form `mimir connector add gmail … auth.kind=oauth …` runs the same flow.

## Privacy

- Mimir only **reads** mail — it never sends, deletes, or marks messages. It uses `BODY.PEEK[]` so your unread mail stays unread.
- All data is fetched and stored locally; no cloud intermediary.
- "Forget everything from Gmail" is exposed through `mimir connector forget <slug>` (A3 / #204): the cascade trashes the connector's facts (recoverable 30 days), credentials, and row, beyond the library-level `forget()` wipe of cursor, buffer, and stored secret.

## Config example

```json
{
  "host": "imap.gmail.com",
  "port": 993,
  "mailbox": "INBOX",
  "auth": { "kind": "app_password", "username": "you@gmail.com" },
  "mode": "auto",
  "llm_extraction_max_attempts": 3
}
```

`mode` can be `"auto"` (default — IDLE if supported, else polling), `"idle"`, or `"poll"`. `llm_extraction_max_attempts` (default 3) bounds how many times the LLM layer retries a message before marking it permanently failed. V1 syncs a single mailbox (`INBOX` by default, configurable).
