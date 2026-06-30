# `mimir-api-types` and `mimir-client`

## Overview

Two new workspace members were introduced in v0.13.0 to decouple the CLI from `mimir-core`:

- **`mimir-api-types`** — Shared serde wire types used by both the server and the HTTP client.
- **`mimir-client`** — Thin HTTP wrapper around `reqwest` that talks to the Mimir daemon.

## Crate Structure

```text
mimir-api-types/
└── src/lib.rs
    ├── ChatRequest
    ├── ChatResponse
    ├── StatusResponse
    ├── Usage
    └── StreamItem

mimir-client/
└── src/lib.rs
    ├── MimirClient
    └── ClientError
```

## `mimir-api-types`

### Rationale

Previously, `mimir-server` defined its own request/response types in `src/types.rs` and `mimir-core` defined `Usage` and `StreamItem` in `src/llm/types.rs`. This meant the CLI binary had to import `mimir-core` (and transitively `sqlx`, `reqwest`, etc.) just to parse JSON responses.

By extracting the wire types into a zero-dependency crate (only `serde`), both the server and the client can share the same schema without pulling in heavy dependencies.

### Types

- **`ChatRequest`** — `session_id`, `message`, `model`, `personality_preset`, `incognito`
- **`ChatResponse`** — `session_id`, `response`, `usage`
- **`StatusResponse`** — Rich health and runtime metadata (see `mimir-server` docs)
- **`Usage`** — `prompt_tokens`, `completion_tokens`, `total_tokens`
- **Sparse-field hygiene (issue #160)** — All `Option<T>` wire fields use
  `#[serde(skip_serializing_if = "Option::is_none")]`, including the
  knowledge-graph wire types (`AuditRow`, `CategoryResponse`,
  `CategoryDetailResponse`, `TrashRow`, `OptimizationStatusResponse`,
  `OptimizationRunNowResponse`, `OptimizationRunSummary`). Sparse rows no longer
  emit `null`, shrinking payloads 42–75% per row.
- **`StreamItem`** — `Text(String)` | `Usage(Usage)` (client-side parsed SSE output)

## `mimir-client`

### Design

`MimirClient` is intentionally thin:

- No retry logic — the daemon handles that.
- No connection pooling beyond `reqwest`'s default — the daemon is local.
- SSE parsing is a lightweight line parser over `reqwest::Response::bytes_stream()` with no extra SSE crate.

### Construction

`MimirClient::new(base_url)` builds a `reqwest::Client` with the default 10 s
connect timeout and 120 s total timeout. It panics if the client cannot be
built; callers that prefer a fallible path (e.g. explicit timeouts, or graceful
startup on misconfigured TLS backends) use `MimirClient::try_new(base_url,
connect_timeout, timeout) -> Result<Self, ClientError>` (issue #165). Build
failures map to `ClientError::Connection`.

`try_new` validates `base_url` up front (it must parse as a base URL) and
strips trailing slashes, so malformed input is rejected and endpoint paths
never contain a double slash.

### Request helpers (DRY)

Every method routes through a small set of private helpers so the
status-check + JSON-decode + error-mapping logic is centralised (issue #167):

- `send_response(req)` — send + status check, returns the raw `reqwest::Response`.
- `send_json::<T>(req)` — `send_response` + `.json::<T>()`, mapping reqwest errors to `ClientError`.
- `get_json::<T, P>(url, &query)` / `post_json::<T, B>(url, &body)` — convenience wrappers.
- `check_status(resp)` — status-only validation returning `Result<(), ClientError>`.

`stop` keeps bespoke status handling because a 503 response (server already
shutting down) must be treated as success.

### SSE Parsing

The parser accumulates bytes into a buffer, splits on `\n\n` (SSE event boundaries), and parses each block:

- `event:` line determines the event type.
- `data:` lines are concatenated.
- No event type or `event: text` → `StreamItem::Text(data)`
- `event: usage` → parse JSON as `Usage` → `StreamItem::Usage`
- `event: error` → `ClientError::Server`

The buffer is capped at 1 MiB (`MAX_SSE_EVENT_SIZE`); a stream that never emits
a delimiter produces `ClientError::Connection("SSE event exceeded max size")`
instead of growing unbounded. The delimiter scan resumes from the last
inspected offset and uses SIMD-accelerated `memchr::memmem`, so accumulation is
linear rather than quadratic (issue #164).

### Methods

| Method | Endpoint | Description |
|--------|----------|-------------|
| `chat` | `POST /chat` | Non-streaming chat completion |
| `chat_stream` | `POST /chat/stream` | Returns an `impl Stream<Item = Result<StreamItem, ClientError>>` |
| `status` | `GET /status` | Rich status metadata |
| `memory` | `GET /memory` | Live condensed memory block from the knowledge graph |
| `stop` | `POST /stop` | Triggers graceful daemon shutdown |

### Error Types

- `Connection` — DNS or TCP failure
- `Http(reqwest::Error)` — HTTP-level error
- `Serialization(serde_json::Error)` — JSON parse failure
- `Server { status, message }` — Non-2xx status or explicit `event: error` in SSE stream

## Session State

With v0.13.0, `mimir chat` is fully stateless. The daemon owns the session and conversation history. The REPL only holds an optional `session_id: Option<String>`:

- First turn: `session_id = None` → server creates a new session → client updates `session_id` from `ChatResponse`.
- Subsequent turns: `session_id = Some(...)` → server continues the existing session.
- `/clear`: `session_id = None` → next turn creates a fresh session.

This means a conversation can be resumed across REPL restarts by reusing the same session ID (stored externally by the user).
