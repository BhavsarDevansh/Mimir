# Connector Rate Limiting & Retry (mimir-connectors)

> **Phase:** 3 — Connectors
> **Issue:** #189 / F12
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md` (decision D′)
> **Landed in:** v0.72.0

## Purpose

Shared rate-limiting and retry/backoff primitives consumed by **every network
connector** so that outbound HTTP / IMAP / CalDAV API calls throttle, cap, and
retry uniformly across backends. One `RateLimitConfig` + `RateLimiter` per
connector instance; one `retry_with_backoff` helper for transient failures.

**Connector LLM calls are exempt** (decision D′): those route through the
shared `LlmWorkerPool` system queue and must *not* be wrapped here. This module
governs service API calls only.

## Why a dedicated primitive

Without it, each backend (Photos, CalDAV, IMAP, the OSM Nominatim geocoder)
would reimplement throttling and 429/503 handling, drifting in behaviour and
re-introducing the same bugs. Centralising it also keeps the LLM-client retry
logic in `mimir-core` (which serves the chat path) decoupled from the
connector-service retry logic (different error domains, different quotas).

## Public API

```rust
pub struct RateLimitConfig {
    pub requests_per_second: f32,
    pub burst_size: u32,
    pub daily_quota: Option<u32>,
    pub backoff_strategy: BackoffStrategy,
}

pub enum BackoffStrategy {
    Exponential { base: Duration, max: Duration, jitter: Duration },
    Linear      { base: Duration, step: Duration, max: Duration, jitter: Duration },
    Fixed       { delay: Duration, jitter: Duration },
}

pub struct RateLimiter { /* built per connector instance */ }

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Result<Self, RateLimitError>;
    pub fn config(&self) -> &RateLimitConfig;
    pub async fn acquire(&self) -> Result<(), RateLimitError>;
}

pub trait Retryable {
    fn retry_hint(&self) -> RetryHint;
}

pub enum RetryHint { Retry { retry_after: Option<Duration> }, Stop }

pub async fn retry_with_backoff<E, F, Fut, T>(
    strategy: &BackoffStrategy,
    max_attempts: u32,
    operation: F,
) -> Result<T, RetryError<E>>;
```

`RateLimitConfig` is `serde`-serialisable; durations use human-readable strings
via `humantime` (`"500ms"`, `"30s"`), so a config embeds directly in a
connector's `config_json`:

```json
{
  "requests_per_second": 1.0,
  "burst_size": 1,
  "daily_quota": null,
  "backoff_strategy": {
    "kind": "exponential",
    "base": "1s",
    "max": "60s",
    "jitter": "250ms"
  }
}
```

## Token bucket

`RateLimiter::acquire` blocks until the GCRA token bucket admits a request,
backed by `governor` (a vetted, `unsafe`-free Generic Cell Rate Algorithm
implementation). The sustained rate is `requests_per_second` (fractional values
supported — `0.5` = one request every two seconds) and `burst_size` is the
number of requests allowed in an instantaneous burst before the sustained rate
applies. Internally the rate becomes "one cell replenished every `1/rps`
seconds, with an independent burst capacity".

## Daily quota

An optional rolling 24h cap layered on top of the token bucket. When the quota
is spent, `acquire` returns `RateLimitError::QuotaExhausted { resets_at }`
**instead of parking a task for up to 24h**. The connector / supervisor treats
this as a non-retryable signal for the current cycle and pauses gracefully —
this composes with the `ConnectorSupervisor`'s existing pause/circuit-breaker
logic rather than fighting its shutdown / trigger preemption. The window is
rolling: it starts on the first request and resets 24h later.

## Retry / backoff

`retry_with_backoff` wraps an async operation and retries it on retryable
errors using the configured `BackoffStrategy` with jitter:

- **Exponential** — `min(base * 2^(attempt-1), max)`
- **Linear** — `min(base + step * (attempt-1), max)`
- **Fixed** — a constant `delay`

`max_attempts` is the *total* attempt budget (the first call is attempt 1). A
non-retryable error returns `RetryError::Terminal` immediately; exhausting the
budget returns `RetryError::Exhausted` with the last error and attempt count.

### Retryable classification

`is_retryable_status` flags `{429, 502, 503, 504}` — matching the transient set
already used by `mimir-core`'s `LlmClient`, so HTTP retry behaviour is
consistent across the codebase. `RetryHint::from_status(status, retry_after)`
classifies a status and carries an optional server-supplied `Retry-After`
through. When present, `Retry-After` overrides the computed backoff delay but is
**clamped to the strategy's `max` cap** (`BackoffStrategy::max_cap`), or a
5-minute default ceiling when the strategy has no `max` (`Fixed`), so an
unreasonable server hint cannot stall a connector task beyond the configured
ceiling. The strategy's jitter is then added on top. Connector backends
implement `Retryable` on their request-error enum and delegate to `from_status`
with a parsed `Retry-After` header.

## Presets

`RateLimitConfig::nominatim()` ships a policy-compliant preset for the OSM
Nominatim geocoder (≤ 1 req/s, no burst, no daily quota, exponential backoff).
The caller is still responsible for sending an identifying `User-Agent`.

## System connections

- **Consumers (planned):** the OSM Nominatim geocoder (#191 / S1), the Photos
  file-watcher/EXIF path (C1–C2), the CalDAV calendar client (C3–C4), and the
  IMAP email client (C5–C7). Each builds a `RateLimiter` from its
  `config_json` and calls `acquire()` before every outbound request, wrapping
  the request in `retry_with_backoff`.
- **Not consumed by:** the `LlmWorkerPool` / `LlmClient` (decision D′) or the
  `ConnectorSupervisor`'s own restart-backoff (which is deterministic and
  governs *task* restarts, not API calls).
- **DB boundary:** unchanged — this module adds no `sqlx` dependency; it is
  pure in-process infrastructure in `mimir-connectors`.

## Validation

`RateLimiter::new` rejects: non-finite or non-positive `requests_per_second`,
zero `burst_size`, zero `daily_quota` (when set), and a rate so large its
reciprocal rounds to a zero replenish interval. All surface as
`RateLimitError::InvalidConfig`.

## Testing

- Inline unit tests (clock-injected, no async-timing flakiness): quota-window
  exhaustion + reset, backoff progression + cap + overflow safety.
- Integration tests: token-bucket burst-then-throttle timing, daily-quota
  exhaustion with `resets_at`, retry success/exhaustion/terminal/retry-after
  honouring, `Retry-After` clamping, config serde round-trips, preset values, status classification.
