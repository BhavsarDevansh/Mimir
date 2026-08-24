# Email Connector

> **Phase:** 3 — Connectors
>
> **Status:** Implemented (library + daemon/CLI) — C5 transport (#199) + C6 structured extraction (#200, calendar invites) + #249 (schema.org JSON-LD deterministic extraction) + C7 LLM extraction (#201, unstructured prose) + the interactive OAuth PKCE login (A4 / #205).

## What it is

The Email connector reads your mailbox (Gmail, Outlook/Hotmail/Office 365, Yahoo Mail, Proton Mail via the Bridge, iCloud Mail — any IMAP server) into Mimir and turns your mail into knowledge-graph facts. It speaks IMAP — the open mail protocol your provider already supports, so it works with any compliant server, no vendor lock-in.

It is a background sync worker that runs in two modes automatically:

- **Push (IMAP IDLE)** — when your server supports it (Gmail, Outlook, and iCloud all do), the connector stays connected and is notified the instant a new email arrives, then fetches just that message. Near-real-time, and very cheap.
- **Polling (fallback)** — for servers without IDLE, the connector checks for new mail every few minutes. The connector detects which mode to use on its own.

## How it works

- You point it at your IMAP server and give it either an **app-specific password** or an **OAuth** token (Google/Microsoft). The secret lives in Mimir's permission-checked secret store (`0600`); the username and server config live in the connector config.
- Each sync fetches only what's new. Mimir tracks the last message it saw by **UID** (a stable, server-assigned message id), so it never re-downloads the same email. Your progress is saved across restarts.
- The **first sync imports your existing inbox**: in push mode Mimir fetches the mail already in the mailbox before it starts listening for new mail, so nothing is missed on day one. When you add a connector with the wizard you can instead choose "only new content from now on" — Mimir then starts from the moment you set it up and leaves older mail alone.
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

### Emails are read in context (#398)

Mimir reads every email with its envelope — when it was sent and received, who it was from and to, and whether it is bulk mail — so the facts it creates are bound by that context:

- **Old mail stays old.** An email from two years ago can never turn into a current "do this now" item. Prose facts from a message are anchored to the email's own date, and an action item with no date of its own expires 30 days after the email — so the older the email, the further in the past its obligations are. A "pay rent" reminder from 2024 is recorded as history, not as something you owe today.
- **Bulk mail is filtered before anything is extracted.** Marketing broadcasters (Mailchimp, HubSpot, and similar) and any mail carrying a `List-Unsubscribe` header are skipped entirely — even when they contain calendar invites or machine-readable receipts — so promotions cannot sneak facts into your knowledge graph. Transactional receipts and bookings from general-purpose delivery services (SendGrid, Mailgun, Postmark, Amazon SES) are still read.
- **Forwarded and misdirected mail is informational.** Mail marked as forwarded (a "Fwd:" subject or the standard "Forwarded message" separator), or mail addressed to someone else that landed in your inbox, is still mined for real facts — but it is never treated as a task for you. Nothing in it can become an action item.
- **Relative dates resolve for real.** The extractor is told the email's sent date and today's date, so "see you Tuesday", "next week", and "overdue" become real timestamps instead of guesses.

### If the LLM layer fails (#262)

Sometimes the LLM cannot read an email — a provider hiccup, a network error, or a message the model refuses to process. Mimir never treats that as "nothing to extract": the `connector_item.remember` hook retries the message with backoff, while any facts the deterministic layers already found are kept. The retry is **bounded and restart-safe**:

- Each message gets a small retry budget (3 attempts by default, configurable via `llm_extraction_max_attempts`) with an increasing wait between attempts, so a stuck message cannot keep burning LLM calls forever.
- Once the budget is exhausted the message is recorded as **permanently failed with the reason** and skipped; it never consumes another LLM call. A re-fetched message in a new mailbox epoch (a `UIDVALIDITY` change) is treated as a new message and gets a fresh chance.
- The retry state is saved with the connector between restarts, so a `mimir stop` or reboot resumes the retry where it left off instead of silently dropping the email. Permanently failed messages (with the reason, capped at 64) and, when the hook's pending queue is full, the rejected messages themselves (raw bytes, capped at 1024, mirroring the queue cap) are persisted; a full queue therefore delays extraction rather than losing the email, and the saved messages are re-staged on the next cycle or after a restart.
- If any messages have permanently failed, the connector reports itself as **degraded** in its health status so the situation is visible, and "forget" clears the recorded failures along with everything else.

This is a library component today (in `mimir-connectors`); the daemon wiring that turns it on for a running `mimir` daemon lands with #202.

## Authentication

- **App password** — best for most providers. Generate an app-specific password in your provider's security settings (Gmail calls them "app passwords"); Mimir uses standard IMAP `LOGIN`. Your username is in the connector config; the password is stored securely.
- **OAuth (Google / Microsoft)** — the connector stores your access + refresh token and refreshes the access token automatically before it expires, so you stay connected without re-authorising. The first token is obtained via the interactive PKCE sign-in flow (A4 / #205): running `mimir connector add` with no arguments lists the supported `(connector_type, backend)` pairs, then the provider presets (issue #400) — selecting **Gmail** pre-fills Google's authorization/token endpoints and scope, selecting **Outlook / Office 365** pre-fills the Microsoft identity platform endpoints and the `IMAP.AccessAsUser.All offline_access` scope — and you bring only your own OAuth client ID from the provider's console (Google Cloud Console / Entra ID app registration). The wizard launches your browser at the printed authorize URL and stores the exchanged token bundle. Yahoo, Proton Mail (Bridge), and iCloud use app passwords only; the flag form `mimir connector add email … auth.kind=oauth …` runs the same OAuth flow.

## Privacy

- Mimir only **reads** mail — it never sends, deletes, or marks messages. It uses `BODY.PEEK[]` so your unread mail stays unread.
- All data is fetched and stored locally; no cloud intermediary.
- "Forget everything from this email connector" is exposed through `mimir connector forget <slug>` (A3 / #204): the cascade trashes the connector's facts (recoverable 30 days), credentials, and row, beyond the library-level `forget()` wipe of cursor, buffer, and stored secret.

## Config example

```json
{
  "host": "imap.gmail.com",
  "port": 993,
  "mailbox": "INBOX",
  "auth": { "kind": "app_password", "username": "you@gmail.com" },
  "mode": "auto",
  "connect_timeout_secs": 10,
  "handshake_timeout_secs": 30,
  "llm_extraction_max_attempts": 3
}
```

`mode` can be `"auto"` (default — IDLE if supported, else polling), `"idle"`, or `"poll"`. `connect_timeout_secs` (default 10) bounds how long Mimir waits to establish the TCP connection, and `handshake_timeout_secs` (default 30) bounds the encrypted handshake and the server's first reply. If your network path stalls, the sync cycle fails cleanly, backs off, and retries later instead of getting stuck forever. `llm_extraction_max_attempts` (default 3) bounds how many times the LLM layer retries a message before marking it permanently failed. V1 syncs a single mailbox (`INBOX` by default, configurable).
