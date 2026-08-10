# Chat Server Architecture

## Overview

The Mimir chat server is an Axum HTTP daemon that runs in-process as part of the `mimir start` command. It exposes chat, status, and memory endpoints over a local REST API.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/status` | Health check and runtime introspection |
| `GET` | `/memory` | Live condensed memory block from the knowledge graph |
| `GET` | `/sessions` | List conversation sessions |
| `GET` | `/sessions/{id}/messages` | Messages for a session from last compaction |
| `POST` | `/chat` | Blocking chat completion |
| `POST` | `/chat/stream` | SSE streaming chat completion |

### Request/Response Schemas

#### `POST /chat`

**Request body:**
```json
{
  "session_id": "optional-existing-uuid",
  "message": "Hello, Mimir!"
}
```

**Response body (success):**
```json
{
  "session_id": "uuid-v4",
  "response": "Hello! How can I help?",
  "usage": {
    "prompt_tokens": 10,
    "completion_tokens": 7,
    "total_tokens": 17
  }
}
```

**Errors:**
- `400` — Invalid JSON body.
- `404` — Session not found (unknown `session_id`).
- `503` — Worker pool queue full. Response includes `Retry-After: 5`.

#### `POST /chat/stream`

**Request body:** same as `/chat`.

**Response:** `text/event-stream`.

Events streamed to the client:
- `data: <text_chunk>` — for each content delta.
- `event: usage\ndata: {"prompt_tokens": …}` — final usage block.
- `event: error\ndata: …` — on mid-stream failure (terminal).

Keep-alive pings are sent every 10 seconds.

#### `GET /status`

**Response body:**
```json
{
  "version": "0.11.0",
  "uptime_seconds": 123,
  "queue_depth_user": 0,
  "queue_depth_system": 0,
  "worker_threads": 1
}
```

#### `GET /memory`

**Response:** Live condensed memory block from the knowledge graph (condensed stable facts + upcoming events).

#### `GET /sessions`

**Response body:**

```json
[
  {
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "created_at": "2024-01-01T00:00:00Z",
    "updated_at": "2024-01-02T00:00:00Z",
    "preview": "Hello, Mimir!"
  }
]
```

Sessions are ordered by `updated_at` descending. `preview` is the most recent user message.

#### `GET /sessions/{id}/messages`

**Response body:**

```json
{
  "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "messages": [
    { "role": "system", "content": "...", "created_at": "2024-01-01T00:00:00Z" },
    { "role": "user", "content": "Hello", "created_at": "2024-01-01T00:00:01Z" },
    { "role": "assistant", "content": "Hi!", "created_at": "2024-01-01T00:00:02Z" }
  ]
}
```

If `compacted_at` is set on the session, only messages with `created_at >= compacted_at` are returned. Otherwise all messages are returned.

**Errors:**
- `404` — Session not found.

## Session Lifecycle

1. **Creation** — If `session_id` is omitted, a new session is created with the current `Personality::system_prompt(memory_content)`.
2. **Validation** — If `session_id` is provided but unknown, `404` is returned immediately.
3. **Persistence** — The user message is appended via `ContextManager::add_user_message`.
4. **Enqueue** — Messages are exported and the request is enqueued in the `LlmWorkerPool`. If the pool is full, a `503` error is returned before the 200 response is committed.
5. **Tool Calls** — If the LLM responds with `tool_calls` (OpenAI function-calling format), each tool is executed via `ToolRegistry`, the results are appended as `role: tool` messages, and a follow-up LLM request is made to obtain the final assistant text. Both the blocking (`/chat`) and streaming (`/chat/stream`) endpoints support this loop. In streaming mode, tool-call deltas are accumulated internally and the final text is streamed after execution.
6. **Storage** — The final assistant response is appended via `ContextManager::add_assistant_message`.
7. **Fact Extraction** — For non-incognito sessions, the fact-extraction pipeline (`KnowledgeGraph::extract_facts`) runs in a background task, parsing the user message for structured facts and inserting them into the knowledge graph. Sensitive facts are gated pending user confirmation.

## Incognito Mode (issue #155)

When a chat request sets `incognito: true`:

- No session is created and neither the user message nor the assistant response is persisted.
- **Write-capable tools are suppressed.** Tools implementing `Tool::is_write_tool() -> true`
  (currently `remember`) are excluded from the exported tool set, and any attempt to
  execute them during an incognito turn returns `ToolError::BlockedIncognito` so no
  facts are written to the knowledge graph. Read-only KG tools remain available.
- The live configuration temperature is applied per request via
  `LlmBackend::with_temperature_override` (issue #80), so hot-reloaded
  `llm.temperature` changes take effect without restarting the daemon.

## Concurrency

Per-session requests are serialised using a `DashMap<String, Arc<Semaphore>>`. Cross-session requests run fully in parallel.

## CORS

`tower_http::cors::CorsLayer` is configured to allow:
- Origins: `http://localhost:8080`, `http://127.0.0.1:8080`, `http://localhost:3000`, `http://127.0.0.1:3000`, `http://localhost:5173`, `http://127.0.0.1:5173`
- Methods: `GET`, `POST`
- Headers: `Content-Type`

## Configuration

The server reads its bind address from `[server].bind_addr` in `~/.config/mimir/config.toml` (default: `127.0.0.1:8080`). See [Configuration](configuration.md) for details.

## Module Layout

`mimir-server` splits the daemon into `app.rs` (router assembly + loopback guard), `server.rs` (startup and background tasks), `shutdown.rs` (signal handling + bounded graceful drain), `state/` (shared `AppState` construction), `routes/` (one module per endpoint family — `chat.rs`, `connectors.rs`, `kb/`, `memory.rs`, `sessions.rs`, `status.rs`, `stop.rs`, `kb_categories.rs`), and `error.rs` (wire error mapping). The KB route family is further split by concern in `routes/kb/` (`query`, `detail`, `browse`, `pending`, `trash`, `forget`, `optimization`, `helpers`, `params`).
