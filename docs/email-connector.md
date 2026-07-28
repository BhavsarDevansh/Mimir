# Email Connector (IMAP) — `mimir-connectors::email`

> **Phase:** 3 — Connectors (C5 / issue #199)
> **Feature flag:** `gmail` (default). Framework + mock stay built without it.
> **Status:** Implemented (library only). Mail parsing + structured fact
> extraction (headers/dates/contacts) is C6 / #200; LLM extraction for
> flights/bookings is C7 / #201; the daemon `AppState` wiring + `mimir
> connector …` CLI land in A1–A3 (#202–#204); the interactive OAuth PKCE login
> is A4 / #206.
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

The Email connector is the third concrete connector backend (after Photos and Calendar). It syncs an IMAP mailbox into Mimir and stages raw RFC 822 messages for the knowledge-graph pipeline. It targets Gmail, Outlook.com / Hotmail, and Apple iCloud Mail — any IMAP4rev1 server — and runs in **`Push`** (IMAP IDLE) mode when the server advertises `IDLE`, falling back to **`Polling`** otherwise.

C5 (#199) delivers the **transport**: an `async-imap`-backed client (`LOGIN` / `AUTHENTICATE XOAUTH2`, `EXAMINE`, `UID FETCH` incremental sync, `IDLE` push), OAuth token **refresh** + app-password auth, a UIDVALIDITY-safe last-UID cursor, and a hand-rolled TCP+rustls TLS handshake. It stages raw messages in an internal buffer; **`extract()` returns no facts yet**. C6 / #200 parses those messages into `NormalizedFact`s (headers/dates/contacts); C7 / #201 adds LLM extraction (flights/bookings).

## Spec corrections (issue #199 vs. implementation)

The issue body was written before the connector framework landed; the following diverge from the literal spec and are intentional:

- **`async-imap 0.11.3`, not 0.11.2.** The spec pins `0.11.2`; the latest stable is `0.11.3` (one patch). Built with `default-features = false` + `runtime-tokio` — the crate's *default* feature is `runtime-async-std`, which would pull `async-std` into the tokio-only workspace.
- **rustls, not async-native-tls.** The spec is silent on TLS. async-imap's `connect()` helper uses `async-native-tls` (system OpenSSL). The workspace standardizes on **rustls** (reqwest `rustls` + `rustls-native-certs`), so the connector hand-rolls the TCP + `tokio-rustls` handshake and feeds the `TlsStream` to `async_imap::Client::new` (which accepts any tokio async stream). The `aws-lc-rs` crypto provider matches the one reqwest already compiles — no second TLS stack or provider enters the tree.
- **Cursor encodes UIDVALIDITY.** The spec says "incremental sync by last UID". A bare last-UID is unsafe: if the mailbox is recreated, `UIDVALIDITY` changes and every prior UID is stale (silent gaps/duplicates). The cursor is `<uid_validity>:<last_uid>` (e.g. `17:42`); a UIDVALIDITY mismatch on `EXAMINE` triggers a full re-fetch.
- **OAuth refresh is hand-rolled (DRY with Calendar).** The `oauth2` crate depends on reqwest 0.12, duplicating the workspace reqwest 0.13 stack. The refresh is a single form-encoded POST on the existing reqwest 0.13, shared via `mimir-connectors::oauth`. The interactive PKCE login that *obtains* the first token is A4 / #206.

## Auth

Two credential kinds, mirroring [`SecretBundle`](connector-secret-store.md):

- **App password** — `LOGIN`. The username lives in `config_json` (non-secret); the password lives in the `SecretStore` under the connector slug as `SecretBundle::AppPassword`.
- **OAuth 2.0** (Gmail / Microsoft) — `AUTHENTICATE XOAUTH2`. The access/refresh tokens live in the `SecretStore` as `SecretBundle::OAuth`; the non-secret client config (`token_endpoint`, `client_id`, optional `client_secret`/`scopes`) **and the account `username`** (embedded in the SASL initial response) live in `config_json`. The connector refreshes an expired access token (within a 60 s skew) before every sync/authenticate/ health call and persists the refreshed bundle back to the store; an unknown expiry does not force a refresh every cycle, and a refresh response that omits `refresh_token` retains the prior one (RFC 6749 §6).

The XOAUTH2 SASL initial client response is `base64("user=<u>\x01auth=Bearer <token>\x01\x01")`, produced by an `async_imap::Authenticator`; a later (error) challenge cancels with an empty reply. Token-endpoint errors report only the HTTP status and parsed `error`/`error_description` — never the raw body (which can echo `client_secret`/`refresh_token`).

## Mode — IDLE vs polling

`mode` defaults to **`auto`**. `authenticate` / `health` run a `CAPABILITY` probe (which they do anyway to validate the credentials) and cache whether the server advertises `IDLE`. `Connector::mode` then returns `Push` when `IDLE` is advertised and `Polling` otherwise — a true automatic polling fallback. The `idle` / `poll` config values force one mode. (`mode()` is called by the supervisor after `authenticate`, so the cached capability is set.)

- **Push (IDLE):** each `sync` connects → `EXAMINE` → `IDLE` → `wait_with_timeout` (default 28 min, RFC 2177's 29-min re-issue with a margin). On `NewData` the connector exits IDLE (`DONE`) and runs an incremental `UID FETCH`; on `Timeout`/`ManualInterrupt` it returns `fetched: 0` and the supervisor loops (re-entering IDLE). The connection is re-established per cycle — simple, robust, and re-issued well within the server's inactivity limit.
- **Polling:** each `sync` connects → `EXAMINE` → incremental `UID FETCH`, and the supervisor waits the poll interval (default 5 min ± 30 s) between cycles.

## Sync protocol

Incremental `UID FETCH <last+1>:* (UID INTERNALDATE BODY.PEEK[])`:

- `BODY.PEEK[]` returns the full RFC 822 message (headers + body) **without** marking it `\Seen`.
- `*` is RFC 3501's "max UID"; when `last+1` exceeds the max, the server may re-return the last message, so returned UIDs `<= last` are filtered (no re-fetch, per #199).
- The cursor advances to `<uid_validity>:<max_uid>` on a full/first sync or when new mail arrived; an incremental cycle that fetched nothing leaves the cursor unchanged (the supervisor skips the no-op write).

Each cycle is one connection; the connector never holds a long-lived IMAP session across awaits (IDLE is contained within a single `sync`).

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

## Dependencies

`async-imap 0.11.3` (`runtime-tokio`), `base64 0.22`, `tokio-rustls 0.26`, `rustls 0.23` (`aws-lc-rs`), `rustls-native-certs 0.8`, `futures 0.3` — all already in the dependency tree (via reqwest/async-imap) or pinned to the workspace's versions. Gated by the `gmail` feature.
