# Geocoder Service (Phase 3 S1 / Issue #191)

> **Status:** Implemented (v0.77.0). Library-only; daemon wiring lands with the Photos connector (C2, wired in v0.81.0 / #196), entity-locations write path (S3), and the Location Search tool (#98).

## Summary

A pluggable geocoding abstraction with an OSM Nominatim default backend: forward geocoding (address / place name → coordinates) and reverse geocoding (latitude / longitude → place). It is shared infrastructure consumed by three Phase 3 paths.

## Crate placement (design decision)

The `Geocoder` trait and its `GeocodeResult` / `GeocodeError` types live in **`mimir-core`**; the `NominatimGeocoder` backend lives in **`mimir-connectors`**.

The issue text says the geocoder "lives in `mimir-connectors`", but the workspace dependency graph is `mimir-core` ← `mimir-knowledge` ← `mimir-connectors` (and `mimir-server` on top of all three). One consumer — the Location Search conversational tool (#98) — is a `mimir-core` tool, and `mimir-core` cannot depend on `mimir-connectors` (a cycle). Placing the trait in the shared base layer lets all three consumers name one type; the concrete HTTP-making backend stays in the service-ingestion crate and is injected from `mimir-server` where needed. This is the standard "trait in base, impl in feature crate" pattern.

## API contract

```rust
#[async_trait]
pub trait Geocoder: Send + Sync {
    async fn forward(&self, query: &str) -> Result<Option<GeocodeResult>, GeocodeError>;
    async fn reverse(&self, latitude: f64, longitude: f64)
        -> Result<Option<GeocodeResult>, GeocodeError>;
}
```

`GeocodeResult` carries `latitude`, `longitude`, `display_name`, `country`, `country_code` (lowercased ISO 3166-1 alpha-2), `alternative_names`, and `short_name` (added v0.81.0 / #196).

### `short_name` (v0.81.0 / #196)

`short_name` is the locality-level label suitable for use as a knowledge-graph `Place` entity name (e.g. "Rome", not "Rome, Metropolitan City of Rome, Italy"). The Nominatim backend derives it from the most specific populated locality field in the `address` block, in descending specificity: `city` → `town` → `village` → `hamlet` → `municipality` → `county` → `state` → `region`, falling back to the first comma-separated segment of `display_name` (trimmed) when no locality is present. `None` only when the backend reports neither a locality nor a usable display name.

Using the locality — not the POI `name` — is what lets the Photos connector (C2) resolve photos taken at different spots in the same city to one `Place` entity so corroboration fires across them, instead of fragmenting into one entity per restaurant/landmark. POI-level detail remains available via `display_name` and `alternative_names` for future vision-tracking queries.

### Result vs `Option`

The issue acceptance says "network failure returns `None` gracefully". The trait distinguishes the two failure modes instead of collapsing them:

- `Ok(None)` — the backend responded successfully but found no match (Nominatim's `[]` array or `{"error": …}` reverse payload).
- `Err(GeocodeError)` — transport, decode, or rate-limit failure.

This keeps the daemon observable (errors are logged) while preserving the "no panic" guarantee. Callers wanting the literal acceptance can map `Err` → `None`.

## Nominatim backend

`NominatimGeocoder` (`mimir-connectors/src/geocoder/`) issues `GET /search` (forward) and `GET /reverse` (reverse) with `format=json&addressdetails=1&namedetails=1`. `lat`/`lon` are returned by Nominatim as strings and parsed to `f64`.

### Throttling + retry (reuses F12)

Each request acquires a token from the shared `RateLimiter` built from `RateLimitConfig::nominatim()` (≤ 1 req/s, no burst). Transient failures (429 / 502 / 503 / 504 and transport errors) are retried via `retry_with_backoff`, honouring a server `Retry-After` header. Quota exhaustion is non-retryable (`GeocodeError::RateLimited`) so the caller pauses rather than hammering the service.

### Configuration

`NominatimConfig` is configurable: base `endpoint` (default the public instance; point at a self-hosted Nominatim for heavy use), descriptive `User-Agent` (required by the Nominatim usage policy), optional `contact_email` appended to the UA, the `RateLimitConfig`, `max_attempts`, and per-request `timeout`.

**User-facing config surface (issue #227, v0.134.0):** the daemon builds the backend from the `[geocoder]` section of `config.toml`:

```toml
[geocoder]
enabled = true
endpoint = "https://nominatim.openstreetmap.org"
# contact_email = "you@example.com"
```

`enabled = false` skips geocoder construction entirely — the knowledge graph and the connector supervisor hold `None` and location facts persist with whatever the producer supplied (the coords-only Photos fallback shape, per issue #250). `endpoint` points at a self-hosted Nominatim instance and `contact_email` is appended to the `User-Agent`; everything `NominatimConfig` exposes beyond these (rate limit, retry budget, timeout, `User-Agent` prefix) keeps its policy-compliant default. The mapping lives in `impl From<&GeocoderConfig> for NominatimConfig` (`mimir-connectors/src/geocoder/mod.rs`), and `mimir-server`'s `init_knowledge_graph` applies it when `enabled` is true. The default endpoint constant lives in `mimir-core::geocoder::DEFAULT_NOMINATIM_ENDPOINT` so the compiled-in config default and the backend cannot drift. Env overrides: `MIMIR_GEOCODER_ENABLED`, `MIMIR_GEOCODER_ENDPOINT`, `MIMIR_GEOCODER_CONTACT_EMAIL` (empty value clears the email).

## Consumers

| Consumer | Issue | Where it consumes |
|----------|-------|------------------|
| Photos connector GPS → place entity | C2 | `mimir-connectors` (direct) |
| Entity locations address → coords | #65 / S3 | injected from `mimir-server` into the write path |
| Location Search conversational tool | #98 (S2, deferred) | `mimir-core` tool, impl injected from `mimir-server` |

## Testing

- `mimir-core` unit tests: `MockGeocoder` round-trips configured results / `None` / errors; `GeocodeResult` serde round-trip.
- `mimir-connectors/tests/geocoder_nominatim.rs`: `wiremock`-backed tests covering forward/reverse parsing, empty-result → `None`, 429-retry-then-success, persistent 503 → `Err(Status)`, non-retryable 404 (no retry), connection refused → `Err(Network)` (no panic), and rate-limiter throttling of consecutive requests.

The backend is always built (not behind a feature flag), consistent with the framework core.
