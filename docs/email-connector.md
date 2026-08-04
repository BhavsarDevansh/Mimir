# Email Connector (IMAP) — `mimir-connectors::email`

> **Phase:** 3 — Connectors (C5 / issue #199)
> **Feature flag:** `gmail` (default). Framework + mock stay built without it.
> **Status:** Implemented (library only). C5 transport (#199) + C6 structured extraction (#200, iMIP calendar invites) + #249 (schema.org JSON-LD deterministic extraction) are done. LLM extraction for flights/bookings/prose is C7 / #201; the daemon `AppState` wiring + `mimir connector …` CLI land in A1–A3 (#202–#204); the interactive OAuth PKCE login is A4 / #206.
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

The Email connector is the third concrete connector backend (after Photos and Calendar). It syncs an IMAP mailbox into Mimir and stages raw RFC 822 messages for the knowledge-graph pipeline. It targets Gmail, Outlook.com / Hotmail, and Apple iCloud Mail — any IMAP4rev1 server — and runs in **`Push`** (IMAP IDLE) mode when the server advertises `IDLE`, falling back to **`Polling`** otherwise.

C5 (#199) delivers the **transport**: an `async-imap`-backed client (`LOGIN` / `AUTHENTICATE XOAUTH2`, `EXAMINE`, `UID FETCH` incremental sync, `IDLE` push), OAuth token **refresh** + app-password auth, a UIDVALIDITY-safe last-UID cursor, and a hand-rolled TCP+rustls TLS handshake. It stages raw messages in an internal buffer. C6 / #200 (`extract()`) runs a **deterministic extraction cascade** over those messages. The cascade has two layers today: layer 1 is iMIP calendar-invite extraction (#200); layer 2 is `schema.org` JSON-LD extraction (#249). C7 / #201 adds the LLM layer for unstructured prose.

## Spec corrections (issue #199 vs. implementation)

The issue body was written before the connector framework landed; the following diverge from the literal spec and are intentional:

- **`async-imap 0.11.3`, not 0.11.2.** The spec pins `0.11.2`; the latest stable is `0.11.3` (one patch). Built with `default-features = false` + `runtime-tokio` — the crate's *default* feature is `runtime-async-std`, which would pull `async-std` into the tokio-only workspace.
- **rustls, not async-native-tls.** The spec is silent on TLS. async-imap's `connect()` helper uses `async-native-tls` (system OpenSSL). The workspace standardizes on **rustls** (reqwest `rustls` + `rustls-native-certs`), so the connector hand-rolls the TCP + `tokio-rustls` handshake and feeds the `TlsStream` to `async_imap::Client::new` (which accepts any tokio async stream). The `aws-lc-rs` crypto provider matches the one reqwest already compiles — no second TLS stack or provider enters the tree.
- **Cursor encodes UIDVALIDITY.** The spec says "incremental sync by last UID". A bare last-UID is unsafe: if the mailbox is recreated, `UIDVALIDITY` changes and every prior UID is stale (silent gaps/duplicates). The cursor is `<uid_validity>:<last_uid>` (e.g. `17:42`); a UIDVALIDITY mismatch on `EXAMINE` triggers a full re-fetch.
- **OAuth refresh is hand-rolled (DRY with Calendar).** The `oauth2` crate depends on reqwest 0.12, duplicating the workspace reqwest 0.13 stack. The refresh is a single form-encoded POST on the existing reqwest 0.13, shared via `mimir-connectors::oauth`. The interactive PKCE login that *obtains* the first token is A4 / #206.

## Auth

Two credential kinds, mirroring [`SecretBundle`](connector-secret-store.md):

- **App password** — `LOGIN`. The username lives in `config_json` (non-secret); the password lives in the `SecretStore` under the connector slug as `SecretBundle::AppPassword`.
- **OAuth 2.0** (Gmail / Microsoft) — `AUTHENTICATE XOAUTH2`, **public-client PKCE only** (the desktop-app flow; no client secret is ever stored or sent). The access/refresh tokens live in the `SecretStore` as `SecretBundle::OAuth`; the non-secret client config (`token_endpoint`, `client_id`, optional `scopes`) **and the account `username`** (embedded in the SASL initial response) live in `config_json`. Confidential-client credentials are not supported: a client secret never appears in `config_json` and never leaves the permission-checked `SecretBundle::OAuth` boundary. The connector refreshes an expired access token (within a 60 s skew) before every sync/authenticate/ health call and persists the refreshed bundle back to the store; an unknown expiry does not force a refresh every cycle, and a refresh response that omits `refresh_token` retains the prior one (RFC 6749 §6).

The XOAUTH2 SASL initial client response is `base64("user=<u>\x01auth=Bearer <token>\x01\x01")`, produced by an `async_imap::Authenticator`; a later (error) challenge cancels with an empty reply. Token-endpoint errors report only the HTTP status and parsed `error`/`error_description` — never the raw body (which can echo the `refresh_token`). The token endpoint must use HTTPS (loopback `http://127.0.0.1` / `::1` / `localhost` is permitted as the local trust boundary); a non-HTTPS remote endpoint is rejected before any credential is posted, and a provider-supplied `error_description` is truncated to 256 bytes so an unbounded payload cannot bloat logs or `last_error`.

## Mode — IDLE vs polling

`mode` defaults to **`auto`**. `authenticate` / `health` run a `CAPABILITY` probe (which they do anyway to validate the credentials) and cache whether the server advertises `IDLE`. `Connector::mode` then returns `Push` when `IDLE` is advertised and `Polling` otherwise — a true automatic polling fallback. The `idle` / `poll` config values force one mode: `idle` errors if the cached capability confirms the server lacks `IDLE`, and `auto` defaults to `Push` (IDLE) before the first probe so `use_idle()` and `mode()` never disagree (a mismatch would let the supervisor's push loop busy-spin on immediate polling returns). (`mode()` is called by the supervisor after `authenticate`, so the cached capability is set.)

- **Push (IDLE):** each `sync` connects → `EXAMINE` → `IDLE` → `wait_with_timeout` (default 28 min, RFC 2177's 29-min re-issue with a margin). On `NewData` the connector exits IDLE (`DONE`) and runs an incremental `UID FETCH`; on `Timeout`/`ManualInterrupt` it returns `fetched: 0` and the supervisor loops (re-entering IDLE). The connection is re-established per cycle — simple, robust, and re-issued well within the server's inactivity limit.
- **Polling:** each `sync` connects → `EXAMINE` → incremental `UID FETCH`, and the supervisor waits the poll interval (default 5 min ± 30 s) between cycles.

## Sync protocol

Incremental `UID FETCH <last+1>:* (UID INTERNALDATE BODY.PEEK[])`:

- `BODY.PEEK[]` returns the full RFC 822 message (headers + body) **without** marking it `\Seen`.
- `*` is RFC 3501's "max UID"; when `last+1` exceeds the max, the server may re-return the last message, so returned UIDs `<= last` are filtered (no re-fetch, per #199).
- The cursor advances to `<uid_validity>:<max_uid>` on a full/first sync or when new mail arrived; an incremental cycle that fetched nothing leaves the cursor unchanged (the supervisor skips the no-op write).
- A missing `UIDVALIDITY` response code on `EXAMINE` is a hard `ConnectorError::Parse` (RFC 3501 mandates the code); it never collapses to epoch `0`, which could collide with a persisted `0:<uid>` cursor and silently skip mail.

Each cycle is one connection; the connector never holds a long-lived IMAP session across awaits (IDLE is contained within a single `sync`).

## C6 / #200 — Structured extraction (iMIP invites)

`extract()` drains the staged RFC 822 messages and runs a **deterministic (structured-parse) extraction cascade** over each. The email is treated as **provenance, not the fact**: the fact is about the real-world thing the email conveys (an appointment), and the email's `UIDVALIDITY`-qualified IMAP UID rides on every fact as the `raw_reference`. The cascade has two layers today — **iMIP calendar invites** (layer 1, below) and `schema.org` JSON-LD extraction (layer 2, #249):

- A MIME attachment with `Content-Type: text/calendar; method=REQUEST | REPLY` is parsed with `mail-parser`, then the embedded `VEVENT` is parsed with the shared `mimir-connectors::ical` module (the same one the Calendar connector uses — DRY) and turned into the same appointment fact cluster the Calendar connector emits: a primary `user has_event <event>` (typed `EventType::Appointment`, recurrence from `RRULE` `FREQ`, temporal bounds from `DTSTART`/`DTEND`), `<event> located_in <place>`, and `<attendee> attending <event>`. Entities resolve via the full F5 chain in `normalize_and_insert`; facts carry `source_type = Connector`, `connector_type = Gmail`, `extraction_method = StructuredParse`.
- Any `text/calendar` MIME part (an attachment or a body part — the walk covers every MIME part, not just those with `Content-Disposition: attachment`) whose `METHOD` is `REQUEST` or `REPLY` is parsed with `mail-parser`, then the embedded `VEVENT` is parsed with the shared `mimir-connectors::ical` module (the same one the Calendar connector uses — DRY) and turned into the same appointment fact cluster the Calendar connector emits: a primary `user has_event <event>` (typed `EventType::Appointment`, recurrence from `RRULE` `FREQ`, temporal bounds from `DTSTART`/`DTEND`), `<event> located_in <place>`, and `<attendee> attending <event>`. The `METHOD` is resolved from the MIME `Content-Type` `method` parameter, falling back to the iCalendar body `METHOD` property (RFC 6047 §2.4 makes the parameter optional); if both are present and disagree the part is rejected as an invalid iMIP object. Only `REQUEST`/`REPLY` are extracted; `PUBLISH` and `CANCEL` are skipped. Entities resolve via the full F5 chain in `normalize_and_insert`; facts carry `source_type = Connector`, `connector_type = Gmail`, `extraction_method = StructuredParse`.
- `method = PUBLISH` (often marketing webinars) and `CANCEL` (deletion lifecycle) are **skipped** for now — `CANCEL` → KB fact lifecycle is tracked in #247.
- **No per-email communication facts are emitted** (`received_email_from` / `sent_email_to`), and **no `Person` entities are auto-created from `From`/`To` headers**, so marketing/spam produces no junk facts. A plain prose email with no `text/calendar` part produces nothing in C6.

The `user has_event` primary fact is authored against the injected canonical user identity (`ConnectorContext::user_identity`, the `config.toml` `[identity] name`), so an invite surfaces in the user's "Upcoming" memory section and resolves to the same entity the daemon treats as `user_entity_id`. Without an identity the primary fact is skipped; the event is still captured via its location/attendee facts. The email connector factory now consumes `ctx.user_identity` (matching the Calendar connector).

### What C6 does *not* do (and why)

A dentist's free-text "see you Tuesday 3pm" confirmation, a flight boarding pass in prose, a bank statement, a job offer — none of these carry structured `text/calendar`, so C6 emits nothing for them. Those are read by the **LLM layer (C7 / #201)**, which reads the body under a strict tool schema and funnels through the *same* `normalize_and_insert` pipeline with `extraction_method = LlmExtraction`. Transactional emails that embed machine-readable `schema.org` JSON-LD (`Order`, `FlightReservation`, `Ticket`, …) are extracted by the deterministic **layer 2** (#249), between invites and the LLM. The cascade is built so those layers slot in without restructuring `extract()`. Duplicate/re-sent invites and re-sent transactional emails dedupe via the existing `normalize_and_insert` corroboration/supersession.

### Layer 2 — `schema.org` JSON-LD extraction (#249)

Many transactional emails (flight confirmations, hotel bookings, e-commerce orders, delivery tracking, event tickets) embed machine-readable `schema.org` JSON-LD in `<script type="application/ld+json">` tags within their HTML body. Layer 2 scans every `text/html` MIME part for these blocks, parses the JSON, flattens the JSON-LD structure (`@graph`, arrays, single objects), and dispatches recognised `schema.org` types to per-type fact extractors:

- **`FlightReservation`** → `user has_flight <flight>` (Event, `Appointment`, temporal = departure → arrival), `<flight> departs_from <airport>` (Place), `<flight> arrives_at <airport>` (Place), `<flight> operated_by <airline>` (Organization). The flight name is `"{airline} {flightNumber}"`, falling back to `"{origin} → {destination}"`.
- **`LodgingReservation`** → `user has_booking <hotel>` (Event, `Appointment`, temporal = checkin → checkout), `<booking> located_in <hotel address>` (Place, only when a distinct address is available — a self-referential `located_in` is skipped).
- **`EventReservation`** → `user has_event <event>` (Event, `Appointment`, temporal = start → end), `<event> located_in <venue>` (Place).
- **`Order`** → `user has_order <order>` (Event, `event_type = None` — an order is a fact, not a scheduled event), `<order> purchased_from <merchant>` (Organization).
- **`ParcelDelivery`** → `user has_delivery <tracking>` (Event, `Reminder` — a delivery is something to track), `<delivery> shipped_by <carrier>` (Organization), `<delivery> delivered_to <address>` (Place).
- **`Ticket`** → `user has_ticket <ticket>` (Event, `event_type = None` — a Ticket is too generic to assume time-bound), `<ticket> issued_by <issuer>` (Organization).
- **`ReservationPackage`** → flattens its `subReservation` array and processes each sub-reservation by its own `@type` (airlines bundle multi-leg flights this way).

Unrecognised `@type` values are logged at `debug` level and skipped — never guessed. The primary user-scoped fact (`user has_flight`, `has_booking`, etc.) is only emitted when a canonical user identity is configured and, for the `Appointment`-typed reservation facts (flight, lodging, event), a parseable start time (`departureTime` / `checkinDate` / `startDate`) is present — an appointment overlay needs a `valid_from`, matching the iMIP layer's `DTSTART` requirement. Identifier fields (`orderNumber`, `trackingNumber`, `ticketNumber`, `flightNumber`, `iataCode`) accept JSON numbers as well as strings. Secondary facts (locations, carriers, merchants) are always emitted so the entity details are captured even without an identity. All facts carry `source_type = Connector`, `extraction_method = StructuredParse`, and the email's `UIDVALIDITY`-qualified IMAP UID as `raw_reference`. No LLM is involved — pure deterministic Rust parsing per the project rule "logic in Rust, not prompts". The implementation lives in `mimir-connectors/src/email/jsonld.rs` and adds no new dependencies (it uses the existing `serde_json`, `mail-parser`, and `chrono`).

## Config (`config_json`)

```json
{
  "host": "imap.gmail.com",
  "port": 993,
  "mailbox": "INBOX",
  "auth": { "kind": "app_password", "username": "you@gmail.com" },
  "mode": "auto",
  "poll_interval_secs": 300,
  "poll_jitter_secs": 30,
  "idle_timeout_secs": 1680,
  "display_name": "Gmail"
}
```

OAuth auth block: `{ "kind": "oauth", "username": "you@gmail.com", "token_endpoint": "https://oauth2.googleapis.com/token", "client_id": "…", "client_secret": "…", "scopes": ["https://mail.google.com/"] }`.

## Testing

The transport is exercised against a minimal scripted IMAP server speaking the protocol over a `tokio::io::duplex` pair — no TLS, no live account. `async_imap::Client::new` accepts any tokio async stream, so the same `imap_login` / `ImapSession` / `run_sync` code paths the daemon runs against a rustls socket run against the fake server. Covered: app-password login, XOAUTH2 SASL initial response, incremental + no-op + full sync, UIDVALIDITY reset full re-sync, IDLE push → fetch, and IDLE timeout → zero.

C6 extraction is unit-tested with fixture `.eml` bytes: the iMIP `REQUEST`/`REPLY` cluster (`has_event` + `located_in` + `attending`, `Appointment` type, temporal bounds), `PUBLISH`/`CANCEL` skipped, plain/marketing email → no facts, and the no-identity path. A knowledge-graph integration test stages an invite, runs `extract()` → `normalize_and_insert`, and asserts F5 entity resolution (user / event / place / attendees), the `Appointment` events-subsystem overlay, secondary facts carrying no overlay, and connector provenance (`Connector` / `Gmail` / `StructuredParse` / `raw_reference` = the `UIDVALIDITY`-qualified IMAP UID). A fake-IMAP → `extract()` round-trip proves the transport and extraction compose end-to-end. The #249 JSON-LD extraction is unit-tested with fixture JSON-LD for each recognised type (flight, lodging, event, order, parcel delivery, ticket, reservation package), the HTML `<script>` scanner (standard, single-quoted, multi-attribute, case-insensitive, JavaScript-skipping, `data-type`-not-`type`), and the JSON-LD structural normalization (`@graph`, arrays, context wrappers). A cascade integration test proves both layers (iMIP + JSON-LD) produce facts from a single email, and a KB integration test verifies flight facts resolve to entities and carry correct connector provenance.

## Dependencies

Transport: `async-imap 0.11.3` (`runtime-tokio`), `base64 0.22`, `tokio-rustls 0.26`, `rustls 0.23` (`aws-lc-rs`), `rustls-native-certs 0.8`, `futures 0.3`. Extraction (C6 / #200, #249): `mail-parser 0.11.5` (RFC 5322 / MIME parsing) is Gmail-specific and gated by the `gmail` feature. The #249 JSON-LD extraction adds no new dependencies — it reuses the existing `serde_json`, `mail-parser`, and `chrono`. The shared `icalendar 0.17` + `chrono-tz 0.10` (`VEVENT` parsing + `TZID` resolution, DRY with the Calendar connector) are gated by both `calendar` and `gmail`; the shared `ical` module is built under `any(feature = "calendar", feature = "gmail")`.
