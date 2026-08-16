# Phase 3: Connectors

> **Status:** Implemented — framework (F1–F10, F12–F13; F11 keyring deferred), all three backends (C1–C7), CLI/server (A1–A4), and testing (T1–T2) have landed. The detailed issue breakdown and design decisions live in `VISION/09-Roadmap/Phase-3-Plan.md`; this file is the high-level deliverable checklist.
>
> **Last Updated:** 2026-08-16

## Goal
Build the connector framework and implement 3 core connectors: Email, Calendar, and Photos.

## Deliverables

### 3.1 Connector Framework
- [x] `Connector` trait definition (F6 / #183)
- [x] Connector registry and discovery (F7 / #184)
- [x] Async task isolation per connector (F8 / #185)
- [x] Shared rate limiter (F12 / #189)
- [x] Error handling and retry logic (F12 / #189)
- [x] Health monitoring (F8 / #185)
- [x] Authentication framework (OAuth, token, local) (F10 / #187, #240, A4 / #205)

### 3.2 Gmail / IMAP Connector
- [x] OAuth 2.0 authentication (C5 / #199)
- [x] IMAP IDLE for real-time sync (C5 / #199)
- [x] Email fetching and parsing (C5 / #199)
- [x] Fact extraction: deterministic cascade with LLM fallback (C6–C7 / #200, #201, #249)
- [x] Extract: flight confirmations, bookings, dates, contacts (C6 / #200, #249, C7 / #201)
- [x] Incremental sync (track last UID) (C5 / #199)

### 3.3 Google Calendar / CalDAV Connector
- [x] OAuth 2.0 or app password auth (C3 / #197)
- [x] Event fetching (single and recurring) (C3 / #197)
- [x] Fact extraction: events, locations, attendees (C4 / #198)
- [x] Write support: add/update/delete events (C4 / #198)
- [x] Incremental sync (track sync token) (C3 / #197)

### 3.4 Photos Connector
- [x] Local file system watcher (C1 / #195)
- [x] EXIF metadata extraction (C1 / #195)
- [x] GPS coordinate parsing (C1 / #195)
- [ ] Thumbnail generation (not in the Phase 3 plan; deferred)
- [x] Fact extraction: locations, dates (C1 / #195, C2 / #196)
- [ ] Fact extraction: objects (basic) (not in the Phase 3 plan; deferred)
- [ ] Optional: Google Photos API integration (follow-on, out of scope)

### 3.5 Normalization Pipeline
- [x] Common schema for all extracted facts (F4 / #181)
- [x] Entity resolution (match to existing entities) (F5 / #182)
- [x] Temporal normalization — typed `valid_from` / `valid_until` bounds, per-backend date parsing (F4 / #181)
- [x] Confidence scoring per source/connector type (F4 / #181)

### 3.6 CLI Management
- [x] `mimir connector add <name>` (A3 / #204)
- [x] `mimir connector remove <name>` (A3 / #204)
- [x] `mimir connector status` (A3 / #204)
- [x] `mimir connector sync <name>` (A3 / #204)
- [x] `mimir connector pause/resume <name>` (A3 / #204)

### 3.7 Testing
- [x] Mock connector for testing framework (F13 / #190)
- [x] Unit tests for each connector
- [x] Integration tests: sync → extract → KB insert (T1 / #206)
- [x] Rate limiting tests (F12 / #189)
- [x] OAuth flow tests (mock server) (T2 / #207)

## Success Criteria
- 3 connectors operational and syncing — met: Photos (C1–C2), Calendar (C3–C4), Email (C5–C7)
- Facts automatically extracted and inserted into KB — met: all backends funnel through the shared `normalize_and_insert` pipeline
- User can add/remove connectors via CLI — met: `mimir connector` subcommands (A3 / #204)
- Incremental sync works (no re-fetching everything) — met: per-connector persisted cursors (Photos file signature, CalDAV sync token, IMAP UIDVALIDITY-safe UID)
- Rate limits respected — partial: shared F12 rate limiter + retry/backoff primitives landed and the geocoder uses them; Calendar CalDAV and Email IMAP outbound calls do not yet

## Dependencies
- Phase 1 (Core Agent) — complete
- Phase 2 (Knowledge Graph) — complete

## Risks
- OAuth complexity (token refresh, scopes, consent screens) — mitigated: vetted `oauth2` crate, interactive PKCE flow (A4 / #205), refresh with 60 s skew and refresh-token retention
- IMAP server variations and edge cases — mitigated: UIDVALIDITY-safe cursor, IDLE with polling fallback, rustls TLS
- EXIF parsing edge cases (different camera formats) — mitigated: `kamadak-exif` multi-container support (JPEG/TIFF/HEIF/PNG/WebP)
- LLM extraction reliability and cost — mitigated: deterministic extraction cascade first, LLM as last resort, spam pre-filter, bounded durable retry ledger (#262)
