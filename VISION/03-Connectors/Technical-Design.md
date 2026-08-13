# Connectors — Technical Design

> **Source of truth:** this document is kept in sync with the locked Phase 3 implementation. The authoritative references are `mimir-connectors/src/connector.rs` (the runtime `Connector` / `ConnectorFactory` traits and their data types), `mimir-knowledge/src/normalize/types.rs` (`NormalizedFact`), and `VISION/09-Roadmap/Phase-3-Plan.md` (the locked architectural decisions A–H). If this document and the code disagree, the code wins — update this document.

## Architecture

Connectors are isolated, plugin-like modules that communicate with the Core Agent via a well-defined interface. Each connector runs in its own task/crate for fault isolation. Connectors are background sync workers that fetch data from an external service, normalize it, and hand it to the knowledge-graph pipeline — they are **not** a parallel track.

### Connector Lifecycle
```
Factory create (config + ConnectorContext) → Authenticate → Sync (periodic) → Extract → Supervisor normalize_and_insert → KB
```

## Interface Definition

The runtime `Connector` trait is the contract every service-ingestion worker implements. It is `#[async_trait]` with a `Send + Sync` supertrait so it is object-safe as `Arc<dyn Connector>` (native `async fn` in traits is not dyn-compatible). Each trait object represents a single configured connector *instance* (one row in the `connectors` table): `id()` is the instance slug, `connector_type()` is the provenance/reliability axis, and `name()` is the display name.

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    /// Stable, unique, slug-style identifier for this connector instance.
    fn id(&self) -> &str;

    /// Human-readable name
    fn name(&self) -> &str;

    /// Provenance and reliability axis (Gmail / Calendar / Photos / …).
    fn connector_type(&self) -> ConnectorType;

    /// How the supervisor should run this connector (polling vs push).
    fn mode(&self) -> ConnectorMode;

    /// Required configuration schema
    fn config_schema(&self) -> serde_json::Value;

    /// Perform (or refresh) authentication with the service. Credentials are
    /// injected at construction; returns the resulting auth state for the
    /// supervisor to persist.
    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError>;

    /// Probe the service's current reachability and auth health.
    async fn health(&self) -> Result<HealthStatus, ConnectorError>;

    /// Fetch raw items from the service into the connector's internal buffer.
    /// Does not extract facts or touch the knowledge graph.
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError>;

    /// Drain buffered raw items into typed, parsed normalized facts. Entity
    /// ids are not resolved here — that is `normalize_and_insert`'s job.
    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError>;

    /// Optional write-back to the service. Default implementation declines.
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError>;

    /// Remove all local data and credentials for this connector instance.
    async fn forget(&self) -> Result<(), ConnectorError>;
}
```

Key differences from the pre-F6 design: `authenticate` takes no arguments (credentials are injected at construction by the factory / secret store, per decision D′), `sync` buffers raw items internally and returns a `SyncOutcome` instead of `Vec<RawEvent>`, `extract` drains that buffer into `Vec<NormalizedFact>` (no `RawEvent` or connector-side `ExtractedFact` type exists — the conversational `ExtractedFact` in `mimir-knowledge::extract` is a separate type), and every method takes `&self` (shared-reference friendly, matching the workspace `Tool` trait) so the trait is callable through the shared `Arc<dyn Connector>` storage used by the registry and supervisor.

### ConnectorFactory

`ConnectorRegistry` maps `(connector_type, backend)` to a `ConnectorFactory`; the factory builds a ready-to-run connector instance from its persisted `config_json` and a shared-services context.

```rust
pub trait ConnectorFactory: Send + Sync {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError>;
}
```

`ConnectorContext` carries the shared services injected at construction: an optional `Arc<dyn Geocoder>` (Photos reverse geocoding, S3), an optional `Arc<dyn SecretStore>` (Calendar / Email credentials, F10), the canonical user identity name, and an optional `Arc<dyn LlmBackend>` (Email LLM extraction, C7, routed through the shared pool's system queue per decision D′).

## Data Types

### ConnectorMode
How the supervisor should run a connector (decision D): `Polling { interval, jitter }` connectors are polled on a fixed interval with random jitter to avoid thundering-herd syncs; `Push` connectors receive events from the service (IMAP IDLE, a file watcher) and are not polled.

### SyncOptions / SyncOutcome
`SyncOptions { full: bool, since: Option<Duration> }` requests a full re-fetch (ignoring the persisted cursor) or an incremental sync with an optional relative time-window hint. `SyncOutcome { fetched: u32, new_cursor: Option<String>, fetched_at: DateTime<Utc> }` reports the number of raw items staged for extraction and the opaque cursor for the supervisor to persist via `KnowledgeGraph::update_sync_cursor`.

### HealthStatus
Transient runtime probe: `Online` / `Offline` / `Degraded` / `AuthExpired` / `NotConfigured`. Deliberately distinct from the persisted lifecycle enums `ConnectorStatus` (`Setup` / `Active` / `Paused` / `Error`) and `ConnectorAuthState` (`Unauthenticated` / `Authenticated` / `Expired`); the supervisor maps a probe onto the persisted columns.

### ConnectorAction / ActionResult
Write-back request (`kind: String` + JSON `payload`) and outcome (`success`, `native_id`, `message`). Backends that do not support write-back leave the default `act` implementation, which returns `ConnectorError::UnsupportedAction`.

### ConnectorError
`thiserror` enum: `Authentication`, `NotAuthenticated`, `Network`, `Config`, `Parse`, `UnsupportedAction`, `Io`, `BackendNotFound`, `BackendAlreadyRegistered`, `Other`. It deliberately does **not** wrap `KnowledgeError`: the connector never performs database inserts (the supervisor owns `normalize_and_insert`), so a connector call never surfaces a knowledge-graph error directly.

### NormalizedFact
The typed, parsed fact produced by `extract()` and consumed by the shared pipeline. Defined in `mimir-knowledge/src/normalize/types.rs` and shared with the conversational `remember` path (decision B). Carries entity *types* (not resolved ids), parsed temporal bounds, typed recurrence, validated category ids, a sensitivity flag, the per-fact `raw_reference` (native item id), an optional per-fact `extraction_method` override, an optional event-type hint, and an optional `NormalizedLocation` overlay. Confidence is **not** carried on the fact — the pipeline assigns it via `confidence::initial(SourceType::Connector, connector_type)`.

## Authentication Patterns

> **Note (updated 2026-07-17, #187 / F10):** the locked Phase 3 plan (`VISION/09-Roadmap/Phase-3-Plan.md`) is the source of truth for credential storage. V1 stores secrets **in plaintext** at rest — one `0600` JSON file per connector under `~/.local/share/mimir/secrets/<slug>.json`, consistent with the plaintext LLM API key in `config.toml` and the home-directory trust boundary. At-rest encryption (`argon2` + `chacha20poly1305`) and an OS keyring backend (`keyring`, #188) are **deferred** follow-ups. The earlier "stored encrypted at rest" wording below is superseded.

### OAuth 2.0 (Gmail, Google Calendar, GitHub, Spotify)
- PKCE flow for native apps
- Token refresh handled automatically
- V1: plaintext `SecretBundle::OAuth` JSON file (mode `0600`); keyring / at-rest encryption deferred (see note above)

### API Tokens (Home Assistant, GitHub PAT)
- User provides token directly
- V1: plaintext `SecretBundle::ApiToken` JSON file (mode `0600`); encryption deferred (see note above)

### Local Discovery (Home Assistant, Photos)
- mDNS/Bonjour discovery
- Local network access only

### Signal
- Use libsignal or signal-cli bridge
- Complex due to E2EE requirements
- May require linking as secondary device

## Sync Strategies

The runtime `ConnectorMode` distinguishes two strategies:

### Polling
- Regular HTTP requests to check for new data
- Configurable interval + random jitter per connector
- Exponential backoff on errors

### Push
- IMAP IDLE for real-time email notification
- Watch directories for new files
- Not polled by the supervisor
- Webhooks (where supported) are a future Push variant

## Rate Limiting & Backoff

Each connector must respect service rate limits. The shared primitives live in `mimir-connectors::rate_limit` (F12 / #189):
```rust
struct RateLimitConfig {
    requests_per_second: f32,
    burst_size: u32,
    daily_quota: Option<u32>,
    backoff_strategy: BackoffStrategy,  // Exponential, linear, fixed
}
```

The framework provides a shared rate limiter and handles 429/503 responses uniformly, honouring `Retry-After` where present.

## Normalization Pipeline

Raw events from different services are wildly different. Each connector's `extract()` converts its buffered raw items into the common `NormalizedFact` schema (illustrated below as JSON); the supervisor then funnels them through the shared `normalize_and_insert` boundary, which resolves entities, assigns confidence, runs the sensitivity gate, and inserts (inheriting corroboration / supersession / inference).

**Email → NormalizedFact:**
```json
{
  "source_type": "Connector",
  "subject": "devansh",
  "subject_type": "Person",
  "relationship_type": "received_email_from",
  "object": "booking@airline.com",
  "object_is_entity": true,
  "object_type": "Organization",
  "valid_from": "2025-05-10T09:00:00Z",
  "is_sensitive": false,
  "category_ids": [],
  "recurrence": "None",
  "requires_user_action": false,
  "raw_reference": "email-uid-12345",
  "extraction_method": "StructuredParse"
}
```

**Photo → NormalizedFact:**
```json
{
  "source_type": "Connector",
  "subject": "devansh",
  "subject_type": "Person",
  "relationship_type": "took_photo_at",
  "object": "Rome",
  "object_is_entity": true,
  "object_type": "Place",
  "valid_from": "2025-05-05T14:30:00Z",
  "is_sensitive": false,
  "category_ids": [],
  "recurrence": "None",
  "requires_user_action": false,
  "raw_reference": "photo-20250505-143000.jpg",
  "extraction_method": "StructuredParse",
  "location": {
    "location_type": "Visited",
    "address": "Rome, Italy",
    "latitude": 41.8902,
    "longitude": 12.4924,
    "timezone": "Europe/Rome"
  }
}
```

**Calendar → NormalizedFact:**
```json
{
  "source_type": "Connector",
  "subject": "devansh",
  "subject_type": "Person",
  "relationship_type": "has_event",
  "object": "Trip to Rome",
  "object_is_entity": false,
  "valid_from": "2025-05-03T00:00:00Z",
  "valid_until": "2025-05-07T23:59:59Z",
  "is_sensitive": false,
  "category_ids": [],
  "recurrence": "None",
  "requires_user_action": false,
  "raw_reference": "caldav-event-42",
  "extraction_method": "StructuredParse",
  "event_type": "Appointment"
}
```

Note: `confidence` and `extraction_method` (when the per-fact override is `None`) come from the batch `Provenance` built by the supervisor, not from the connector.

## Error Handling

Connectors must be resilient:
- Network failures → Retry with backoff
- Authentication expired → Notify user, pause sync
- Malformed data → Log and skip, do not crash
- Service downtime → Queue for retry

## Technology Stack
- **Language:** Rust (edition 2024, `#![deny(unsafe_code)]`)
- **HTTP Client:** reqwest 0.13 (rustls, no OpenSSL system dependency)
- **OAuth:** oauth2 5.0.0 (`default-features = false`) with a custom `AsyncHttpClient` adapter (`OAuthHttpClient`) over the workspace reqwest client
- **Async traits:** async-trait
- **Errors:** thiserror
- **Database:** no direct sqlx — persistence only via the `KnowledgeGraph` facade
- **Serialization:** serde + custom schemas per connector
