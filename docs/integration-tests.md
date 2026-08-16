# Integration Tests

## Server Integration Tests (`mimir-server/src/lib.rs`)

These tests exercise the full Axum HTTP stack without making real network calls. They use `tower::ServiceExt::oneshot` to send requests through the router and `MockLlmClient` to simulate the LLM backend.

### Test Matrix

| Test | Endpoint | Mock Behaviour | Expected Result |
|------|----------|----------------|-----------------|
| `test_status_returns_ok` | `GET /status` | default mock | HTTP 200 |
| `test_chat_creates_session` | `POST /chat` | `push_chat("Hello!", ...)` | HTTP 200, `session_id` present, response text correct |
| `test_chat_stream_returns_sse` | `POST /chat/stream` | `push_stream([Text("Hi"), Usage(...)])` | HTTP 200, `content-type: text/event-stream`, body contains "Hi" |
| `test_chat_llm_error_returns_500` | `POST /chat` | `push_chat_error(Api { status: 500 })` | HTTP 500 |
| `test_chat_stream_llm_error_sends_error_event` | `POST /chat/stream` | `push_stream([Text("partial"), Err(Api {...})])` | HTTP 200 SSE stream containing `error` event |
| `test_chat_queue_full_returns_503` | `POST /chat` | `push_chat_error(QueueFull)` | HTTP 503, `Retry-After: 5` |
| `test_status_returns_queue_depths` | `GET /status` | `user_queue_depth(2)`, `system_queue_depth(1)`, `worker_threads(4)` | JSON with matching queue depths |
| `test_chat_unknown_session_returns_404` | `POST /chat` | default mock | HTTP 404 |
| `test_memory_returns_content` | `GET /memory` | default mock | HTTP 200, body contains memory text |

All tests use a temporary directory for the SQLite context database and a fresh `MockLlmClient` instance so they are fully isolated and parallel-safe.

## Wiremock HTTP Tests (`mimir-core/tests/llm_http_integration.rs`)

These tests verify the **real** `LlmClient` HTTP layer: request serialisation, retry logic, SSE parsing, and error mapping. They use the `wiremock` crate to stand up a real HTTP server on a random localhost port.

### Test Matrix

| Test | Wiremock Setup | Client Call | Assertion |
|------|---------------|-------------|-----------|
| `test_retry_on_429` | 429 then 200 | `chat()` | retries once, succeeds |
| `test_no_retry_on_400` | 400 | `chat()` | fails immediately with `LlmError::Api` |
| `test_sse_stream_parsing` | SSE body with text + usage chunks | `chat_stream_with_usage()` | yields `Text("Hello")` then `Usage(...)` |
| `test_connection_failure` | no server on `127.0.0.1:1` | `chat()` | returns `Network` or `RetryExhausted` |

Security: wiremock binds only to `127.0.0.1`; no API keys are present in test code.
