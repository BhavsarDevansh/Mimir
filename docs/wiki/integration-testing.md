# Integration Testing

## Fast Server Tests

The `mimir-server` crate contains integration tests that exercise the full HTTP router using `MockLlmClient`. These tests run in milliseconds and cover:

- Chat creation and streaming
- Error paths (500, 503, 404)
- SSE content-type headers
- Queue-depth introspection on `/status`

## HTTP-Level Tests

The `mimir-core` crate contains integration tests that use `wiremock` to verify the real HTTP client behaves correctly:

- Retry logic on 429
- No retry on 400
- SSE stream parsing
- Connection failure handling

## Running the Tests

```bash
# All workspace tests
cargo test --workspace

# Integration tests only
cargo test -p mimir-core --test llm_http_integration

# Server tests
cargo test -p mimir-server --lib
```
