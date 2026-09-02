# LLM Client

## Overview

The LLM client is Mimir's interface to OpenAI-compatible language-model APIs. It lives in `mimir-core/src/llm/` and supports both streaming (SSE) and non-streaming chat completion requests with automatic retry on transient failures.

## Module Structure

```text
mimir-core/src/llm/
├── mod.rs              # Public exports
├── types.rs            # Request/response types and errors
├── backend.rs          # `LlmBackend` trait + mock backend
├── client/             # HTTP client with retry logic
│   ├── mod.rs          # `LlmClient` facade, exports
│   ├── construct.rs    # construction, base URL, timeout wiring
│   ├── chat.rs         # chat completion requests (streaming + blocking)
│   ├── transport.rs    # reqwest transport + SSE parsing
│   └── tests.rs        # unit tests
├── mock.rs             # deterministic mock backend for tests
└── pool/               # priority-based worker pool
    ├── mod.rs          # `LlmWorkerPool` facade
    ├── queue.rs        # priority queue
    └── worker.rs       # worker task
```

## Dependencies (verified via Context7 + crates.io)

| Crate | Version | Features | Purpose |
|-------|---------|----------|---------|
| reqwest | 0.13 | `json`, `stream` | Async HTTP client |
| tokio | 1 | `full` | Async runtime |
| eventsource-stream | 0.2.3 | — | Parse SSE streams from `reqwest::bytes_stream()` |
| futures | 0.3 | — | Stream combinators |
| bytes | 1.9 | — | Byte buffers for streaming |
| serde_json | 1.0.149 | — | JSON (de)serialization |
| tracing | 0.1.44 | — | Structured logging |

> **Note:** reqwest 0.13 uses rustls by default; no explicit TLS feature flag is required.

## Types

### `ChatRequest`

OpenAI-compatible chat completion request. Builder methods: `with_max_tokens`, `with_temperature`, `with_stream`.

### `Message`

A chat message with `role` and `content`. Constructors: `Message::system()`, `Message::user()`, `Message::assistant()`.

### `ChatResponse`

Non-streaming response containing `choices` and optional `usage` statistics.

### `Usage`

Token counters: `prompt_tokens`, `completion_tokens`, `total_tokens`.

### `StreamChunk` / `StreamChoice` / `Delta`

Streaming (SSE) response fragments. `Delta::content` holds the incremental text.

### `LlmError`

Structured error enum:
- `Network(reqwest::Error)` — timeouts, DNS, connection failures
- `Api { status, body }` — non-success HTTP response
- `Parse(serde_json::Error)` — malformed JSON
- `RetryExhausted { attempts, last_error }` — all retries failed; `last_error` preserves the final underlying failure (e.g. provider `503` overload) so callers can surface the actionable cause
- `StreamError(String)` — invalid SSE event
- `ClientBuild(String)` — the `reqwest::Client` or worker pool could not be constructed at startup

## Client (`LlmClient`)

### `async fn new(config: LlmConfig) -> Result<Self, LlmError>`

Constructs a client from configuration. Must be called from within a Tokio runtime context because it spawns the internal worker pool. A failure to build the `reqwest::Client` or initialise the worker pool surfaces as `LlmError::ClientBuild` instead of panicking, so daemon startup (`start_server`, `AppState::from_config`) can report the problem and exit cleanly via the normal error path (issue #166).

The underlying `reqwest::Client` uses a 30-second **connect timeout** rather than a global request timeout, so that long-lived SSE streaming responses are not prematurely aborted.

### `async fn new_with_retry_config(config: LlmConfig, retry_config: RetryConfig) -> Result<Self, LlmError>`

Constructs a client with the same runtime requirements and connection timeout as `new`, but overrides the retry schedule for direct calls and for every worker in the client's pool. `RetryConfig` contains `max_attempts` (the total attempt count, including the initial attempt), `base_backoff`, and `max_backoff`; `RetryConfig::default()` preserves the production schedule of four attempts, a 200 ms base, and a 10-second ceiling.

`new_direct(config, retry_config)` (used internally by pool workers) is also fallible. The `reqwest::Client` is built **before** the worker pool spawns, and each worker's client is built up front inside `LlmWorkerPool::new`; the first build failure propagates as `LlmError::ClientBuild` so the pool can never start in a zero-live-worker state.

### `chat(messages) -> Result<(String, Usage), LlmError>`

Sends a non-streaming request and returns the assistant's reply plus token usage.

### `chat_stream(messages) -> Result<Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>, LlmError>`

Sends a streaming request and returns a pinned stream of text chunks.

## Retry Policy

Manual exponential backoff (no extra middleware crates):
- **Max retries:** 3 (4 total attempts)
- **Base delay:** 200 ms
- **Growth:** `200 * 2^attempt` ms (the historical schedule doubles before the first retry)
- **Cap:** 10 s
- **Transient conditions:** timeouts, connection errors, HTTP 429 / 502 / 503 / 504

Retry is applied to the initial HTTP POST only. Individual SSE events are not retried.

## Data Flow

Non-streaming:

```text
User → LlmClient::chat()
       POST /chat/completions
       Retry loop (if transient)
       Deserialize ChatResponse
       Return (content, usage)
```

Streaming:

```text
User → LlmClient::chat_stream()
       POST /chat/completions (stream=true)
       Retry loop (if transient)
       Wrap bytes_stream() in Eventsource
       Yield Ok(chunk) per SSE event
       Filter empty / [DONE] chunks
```

## Testing

Unit tests cover:
- Request JSON serialization
- Message constructors
- SSE chunk parsing
- Backoff calculation (exponential growth + cap)
- Retry exhaustion with the default and injected schedules

## Future Extensions

- Tool-calling (function calling) support will be added when the Tool Registry is implemented.
- Fallback endpoint switching is planned for Phase 2+.
