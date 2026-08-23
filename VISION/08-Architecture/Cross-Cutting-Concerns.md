# Cross-Cutting Concerns

## Security

### Authentication
- Connector credentials are stored **plaintext at rest** in V1 — one `0600` JSON file per connector under `~/.local/share/mimir/secrets/<slug>.json` (parent directory `0700`), consistent with the plaintext LLM API key in `config.toml` and the home-directory trust boundary (Phase 3 plan, #187 / F10)
- An OS-keychain backend (`secrets.backend = "keychain"`, feature `secrets-keyring`, #188) stores the same bundles in the OS keychain; at-rest encryption (`argon2` + `chacha20poly1305`) is a deferred follow-up
- Credentials never enter `config_json` — OAuth client secrets travel in the credential bundle and stored bundles live in the dedicated secrets directory, not in config files
- OAuth tokens refreshed automatically, never logged

### Authorization
- Each connector operates with minimal permissions
- Read-only by default for destructive services (email, bank)
- Write permissions require explicit user grant
- Audit log of all write operations

### API Security
- Core Agent HTTP API binds to localhost only by default
- Optional: TLS with client certificates for remote access
- Rate limiting on all endpoints
- No API keys exposed in process lists (environment variables only)

## Privacy

### Local-First Architecture
- Knowledge Graph stored locally (SQLite)
- Raw data from connectors kept only as needed
- Embeddings and extracted facts are the primary storage
- User can export and delete all data instantly

### Data Minimization
- Only extract facts relevant to user queries and patterns
- Configurable retention policies per connector
- "Forget everything from a connector" is a first-class operation
- PII detection and optional redaction

### Consent Model
- Each connector declares what data it accesses
- User approves each connector individually
- Granular permissions: "read emails" vs "read and write calendar"
- Revocable at any time

## Error Handling

### Connector Resilience
- Network failures: retry with exponential backoff (max 5 attempts)
- Authentication failures: pause sync, notify user
- Malformed data: log and skip, never crash
- Service downtime: queue for retry, backoff increases

### Reasoning Engine Resilience
- LLM API failures: fallback to cached answers, degrade gracefully
- No evidence found: state uncertainty clearly, do not hallucinate
- Timeout: return best-effort answer with partial results

### User Experience
- All errors surfaced in human-readable form
- Suggested fixes included where possible
- "Why did this fail?" is always answerable via audit trail

## Audit Logging

Every significant action is logged:
```rust
struct AuditLog {
    timestamp: DateTime,
    action: String,           // e.g., "kb_insert", "connector_sync", "proactive_notify"
    actor: String,            // "user", "agent", "connector:{id}"
    target: String,           // e.g., entity_id, connector_id
    details: serde_json::Value,
    outcome: Outcome,         // Success | Failure | Partial
}
```

Logs are append-only and stored in SQLite.

## Observability

### Metrics
- Connector sync latency and success rate
- LLM API token usage and latency
- Knowledge Graph query performance
- Reasoning Engine investigation depth and duration
- Proactive Agent suggestion acceptance rate

### Health Checks
```bash
$ agent health
Core Agent:     ● Healthy
Knowledge Graph: ● Healthy (12,304 entities, 48,291 facts)
Connectors:
  Email:        ● Healthy (last sync: 2m ago)
  Calendar:     ● Healthy (last sync: 5m ago)
  Photos:       ● Healthy (last sync: 1h ago)
Reasoning:      ● Healthy
Proactive:      ● Healthy (3 suggestions today, 2 accepted)
Vision:         ○ Not configured

LLM API:        ● Responsive (latency: 245ms)
```

## Backup and Recovery

### Automated Backups
- Daily incremental backup of Knowledge Graph
- Weekly full backup
- Backups stored in `~/.local/share/agent/backups/`
- Optional: encrypted cloud backup (user-configured)

### Recovery
```bash
$ agent restore --from backup-2025-05-19.sql
Restoring Knowledge Graph from 2025-05-19...
✅ Restored 12,304 entities, 48,291 facts
```
