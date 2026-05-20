# Phase 3: Connectors

## Goal
Build the connector framework and implement 3 core connectors: Email, Calendar, and Photos.

## Duration
6–8 weeks

## Deliverables

### 3.1 Connector Framework
- [ ] `Connector` trait definition
- [ ] Connector registry and discovery
- [ ] Async task isolation per connector
- [ ] Shared rate limiter
- [ ] Error handling and retry logic
- [ ] Health monitoring
- [ ] Authentication framework (OAuth, token, local)

### 3.2 Gmail / IMAP Connector
- [ ] OAuth 2.0 authentication
- [ ] IMAP IDLE for real-time sync
- [ ] Email fetching and parsing
- [ ] Fact extraction (LLM-based for structured data)
- [ ] Extract: flight confirmations, bookings, dates, contacts
- [ ] Incremental sync (track last UID)

### 3.3 Google Calendar / CalDAV Connector
- [ ] OAuth 2.0 or app password auth
- [ ] Event fetching (single and recurring)
- [ ] Fact extraction: events, locations, attendees
- [ ] Write support: add/update/delete events
- [ ] Incremental sync (track sync token)

### 3.4 Photos Connector
- [ ] Local file system watcher
- [ ] EXIF metadata extraction
- [ ] GPS coordinate parsing
- [ ] Thumbnail generation
- [ ] Fact extraction: locations, dates, objects (basic)
- [ ] Optional: Google Photos API integration

### 3.5 Normalization Pipeline
- [ ] Common schema for all extracted facts
- [ ] Entity resolution (match to existing entities)
- [ ] Temporal normalization (parse all date formats)
- [ ] Confidence scoring per extraction method

### 3.6 CLI Management
- [ ] `agent connector add <name>`
- [ ] `agent connector remove <name>`
- [ ] `agent connector status`
- [ ] `agent connector sync <name>`
- [ ] `agent connector pause/resume <name>`

### 3.7 Testing
- [ ] Mock connector for testing framework
- [ ] Unit tests for each connector
- [ ] Integration tests: sync → extract → KB insert
- [ ] Rate limiting tests
- [ ] OAuth flow tests (mock server)

## Success Criteria
- 3 connectors operational and syncing
- Facts automatically extracted and inserted into KB
- User can add/remove connectors via CLI
- Incremental sync works (no re-fetching everything)
- Rate limits respected

## Dependencies
- Phase 1 (Core Agent)
- Phase 2 (Knowledge Graph)

## Risks
- OAuth complexity (token refresh, scopes, consent screens)
- IMAP server variations and edge cases
- EXIF parsing edge cases (different camera formats)
- LLM extraction reliability and cost
