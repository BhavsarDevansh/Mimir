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

All tests use a temporary directory for the SQLite context database and a fresh `MockLlmClient` instance so they are fully isolated and parallel-safe. The shared server fixture creates the knowledge-graph database by copying a pre-migrated SQLite template, so every test still receives a clean database without re-running the 60 migrations.

### Server Test Waits (issue #534)

Server integration tests avoid fixed wall-clock waits when synchronising with asynchronous server work. The shared `poll_until` helper probes the asserted observable state—hook running status, job-queue running status, connector supervisor state, or persisted conversation contents—at 10 ms intervals within a bounded timeout, wrapping each predicate evaluation in the remaining budget and capping the polling delay to that budget. `HookEngine::is_running(hook_id)` provides hook-specific readiness, so shared hook waits do not accidentally observe an unrelated running hook. The chat-hook readiness helper additionally uses `HookEngine::is_settled_for(hook_id)` so a failed run cannot appear idle while it is moving from the running map back into the pending queue. If the state is not observed before the timeout, the test fails with the same intent it had before, but without sleeping through a fixed interval on every run.

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

## Connector Test Waits (issue #532)

Connector integration tests avoid fixed wall-clock waits. Where the mock machinery exposes a deterministic event, they await it directly: `MockSyncRecorder::wait_for_completed` observes the RAII guard that records every sync return (including failures and cancellation), while supervisor and OAuth tests await existing readiness, drop, or socket-accept events.

When a test must observe a knowledge-graph or supervisor state that has no event API, it uses the shared bounded `wait_until_some` / `wait_for_async` helpers. The helpers poll at 10 ms and fail with a descriptive timeout. `wait_until_some` returns the first observed value; `wait_for_async` waits until the predicate returns `true`.
