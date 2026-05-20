# Connectors — Technical Design

## Architecture

Connectors are isolated, plugin-like modules that communicate with the Core Agent via a well-defined interface. Each connector runs in its own task/crate for fault isolation.

### Connector Lifecycle
```
Register → Authenticate → Discover → Sync (periodic) → Extract → Normalize → Push to KB
```

## Interface Definition

```rust
#[async_trait]
trait Connector: Send + Sync {
    /// Unique identifier (e.g., "gmail", "homeassistant")
    fn id(&self) -> &str;
    
    /// Human-readable name
    fn name(&self) -> &str;
    
    /// Required configuration schema
    fn config_schema(&self) -> serde_json::Value;
    
    /// Authenticate with the service
    async fn authenticate(&mut self, config: ConnectorConfig) -> Result<AuthState>;
    
    /// Check if currently authenticated and healthy
    async fn health(&self) -> Result<HealthStatus>;
    
    /// Perform a sync (full or incremental)
    async fn sync(&self, options: SyncOptions) -> Result<Vec<RawEvent>>;
    
    /// Extract structured facts from raw events
    async fn extract(&self, events: Vec<RawEvent>) -> Result<Vec<ExtractedFact>>;
    
    /// Optional: write an action back to the service
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult>;
    
    /// Clean up all local data for this connector
    async fn forget(&mut self) -> Result<()>;
}
```

## Data Types

### RawEvent
The unnormalized output from a connector.
```rust
struct RawEvent {
    connector_id: String,
    event_id: String,       // Service-native ID
    event_type: String,     // e.g., "email_received", "photo_taken", "event_created"
    payload: serde_json::Value,  // Raw service data
    timestamp: DateTime,
    fetched_at: DateTime,
}
```

### ExtractedFact
Normalized facts ready for the Knowledge Graph.
```rust
struct ExtractedFact {
    subject: String,        // Entity reference or ID
    predicate: String,
    object: FactObject,     // Entity reference or literal
    temporal: TemporalBounds,
    confidence: f32,
    extraction_method: String, // e.g., "llm_extraction", "structured_parse", "heuristic"
    raw_reference: String,  // Points back to RawEvent
}
```

## Authentication Patterns

### OAuth 2.0 (Gmail, Google Calendar, GitHub, Spotify)
- PKCE flow for native apps
- Token refresh handled automatically
- Secure local storage of tokens (keyring or encrypted file)

### API Tokens (Home Assistant, GitHub PAT)
- User provides token directly
- Stored encrypted at rest

### Local Discovery (Home Assistant, Photos)
- mDNS/Bonjour discovery
- Local network access only

### Signal
- Use libsignal or signal-cli bridge
- Complex due to E2EE requirements
- May require linking as secondary device

## Sync Strategies

### Polling
- Regular HTTP requests to check for new data
- Configurable interval per connector
- Exponential backoff on errors

### Webhooks (where supported)
- Register webhook URL with service
- Receive push notifications
- More efficient than polling

### IMAP/Push (Email)
- IMAP IDLE for real-time email notification
- Fallback to polling

### File System Watchers (Photos)
- Watch directories for new files
- Extract EXIF metadata immediately

## Rate Limiting & Backoff

Each connector must respect service rate limits:
```rust
struct RateLimitConfig {
    requests_per_second: f32,
    burst_size: u32,
    daily_quota: Option<u32>,
    backoff_strategy: BackoffStrategy,  // Exponential, linear, fixed
}
```

The Connector Framework provides a shared rate limiter and handles 429/503 responses uniformly.

## Normalization Pipeline

Raw events from different services are wildly different. The Normalizer converts them to a common schema:

**Email → ExtractedFact:**
```json
{
  "subject": "devansh",
  "predicate": "received_email_from",
  "object": "booking@airline.com",
  "temporal": { "at": "2025-05-10T09:00:00Z" },
  "confidence": 1.0,
  "extraction_method": "structured_parse"
}
```

**Photo → ExtractedFact:**
```json
{
  "subject": "devansh",
  "predicate": "took_photo_at",
  "object": { "lat": 41.8902, "lon": 12.4924 },
  "temporal": { "at": "2025-05-05T14:30:00Z" },
  "confidence": 0.95,
  "extraction_method": "exif_gps"
}
```

**Calendar → ExtractedFact:**
```json
{
  "subject": "devansh",
  "predicate": "has_event",
  "object": "Trip to Rome",
  "temporal": { "from": "2025-05-03", "to": "2025-05-07" },
  "confidence": 1.0,
  "extraction_method": "structured_parse"
}
```

## Error Handling

Connectors must be resilient:
- Network failures → Retry with backoff
- Authentication expired → Notify user, pause sync
- Malformed data → Log and skip, do not crash
- Service downtime → Queue for retry

## Technology Stack
- **Language:** Rust
- **HTTP Client:** reqwest
- **OAuth:** oauth2 crate
- **Database:** Shared SQLite via Knowledge Graph
- **Serialization:** serde + custom schemas per connector
