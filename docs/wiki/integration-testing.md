# Integration Testing

## Fast Server Tests

The `mimir-server` crate contains integration tests that exercise the full HTTP router using `MockLlmClient`. These tests run in milliseconds and cover:

- Chat creation and streaming
- Error paths (500, 503, 404)
- SSE content-type headers
- Queue-depth introspection on `/status`

Server tests synchronise with asynchronous work by polling observable state through the shared `poll_until` helper: hook or job readiness, connector runner state, and persisted message rollbacks. The helper checks every 10 ms inside a bounded timeout, so tests avoid fixed sleeps without silently continuing when the expected state never appears. It returns `false` on expiry and the calling test turns that into a descriptive assertion failure.

## HTTP-Level Tests

The `mimir-core` crate contains integration tests that use `wiremock` to verify the real HTTP client behaves correctly:

- Retry logic on 429
- No retry on 400
- SSE stream parsing
- Connection failure handling

## Connector Tests

Connector tests use deterministic completion signals rather than fixed sleeps. The shared mock recorder can wait for a specific number of completed syncs, and tests that must observe knowledge-graph state use bounded polling with a 10 ms interval; `wait_until_some` returns the first observed value, while `wait_for_async` waits for its predicate to become true.

This makes the connector suite faster and less likely to flake on loaded CI runners while preserving the same assertions.

## Running the Tests

```bash
# All workspace tests
cargo test --workspace

# Integration tests only
cargo test -p mimir-core --test llm_http_integration

# Server tests
cargo test -p mimir-server --lib
```
