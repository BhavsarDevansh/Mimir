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
- **`StreamItem`** — `Text(String)` | `Usage(Usage)` (client-side parsed SSE output)

## `mimir-client`

### Design

`MimirClient` is intentionally thin:

- No retry logic — the daemon handles that.
- No connection pooling beyond `reqwest`'s default — the daemon is local.
- SSE parsing is a lightweight line parser over `reqwest::Response::bytes_stream()` with no extra SSE crate.

### SSE Parsing

The parser accumulates bytes into a buffer, splits on `\n\n` (SSE event boundaries), and parses each block:

- `event:` line determines the event type.
- `data:` lines are concatenated.
- No event type or `event: text` → `StreamItem::Text(data)`
- `event: usage` → parse JSON as `Usage` → `StreamItem::Usage`
- `event: error` → `ClientError::Server`

### Methods

| Method | Endpoint | Description |
|--------|----------|-------------|
| `chat` | `POST /chat` | Non-streaming chat completion |
| `chat_stream` | `POST /chat/stream` | Returns an `impl Stream<Item = Result<StreamItem, ClientError>>` |
| `status` | `GET /status` | Rich status metadata |
| `memory` | `GET /memory` | Plain-text contents of `memory.md` |
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
