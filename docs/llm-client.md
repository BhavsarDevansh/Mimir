# LLM Client

## Overview

The LLM client is Mimir's interface to OpenAI-compatible language-model APIs. It lives in `mimir-core/src/llm/` and supports both streaming (SSE) and non-streaming chat completion requests with automatic retry on transient failures.

## Module Structure

```text
mimir-core/src/llm/
├── mod.rs      # Public exports
├── types.rs    # Request/response types and errors
└── client.rs   # HTTP client with retry logic
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
- `RetryExhausted { attempts }` — all retries failed
- `StreamError(String)` — invalid SSE event
- `ClientBuild(String)` — the `reqwest::Client` or worker pool could not be constructed at startup

## Client (`LlmClient`)

### `async fn new(config: LlmConfig) -> Result<Self, LlmError>`

Constructs a client from configuration. Must be called from within a Tokio runtime context because it spawns the internal worker pool. A failure to build the `reqwest::Client` or initialise the worker pool surfaces as `LlmError::ClientBuild` instead of panicking, so daemon startup (`start_server`, `AppState::from_config`) can report the problem and exit cleanly via the normal error path (issue #166).

The underlying `reqwest::Client` uses a 30-second **connect timeout** rather than a global request timeout, so that long-lived SSE streaming responses are not prematurely aborted.

`new_direct(config)` (used internally by pool workers) is also fallible: a worker that cannot build its HTTP client logs the error and exits rather than aborting the pool.

### `chat(messages) -> Result<(String, Usage), LlmError>`

Sends a non-streaming request and returns the assistant's reply plus token usage.

### `chat_stream(messages) -> Result<Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>, LlmError>`

Sends a streaming request and returns a pinned stream of text chunks.

## Retry Policy

Manual exponential backoff (no extra middleware crates):
- **Max retries:** 3 (4 total attempts)
- **Base delay:** 200 ms
- **Growth:** `200 * 2^attempt` ms
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
- Retry exhaustion on persistent failure

## Future Extensions

- Tool-calling (function calling) support will be added when the Tool Registry is implemented.
- Fallback endpoint switching is planned for Phase 2+.
