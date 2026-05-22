# Chat Server Architecture

## Overview

`mimir-server` is an Axum HTTP daemon that exposes Mimir's chat capabilities over a local REST API. It binds to `127.0.0.1:8080` by default and is intended for single-user, local-first operation.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/status` | Health check and runtime introspection |
| `GET` | `/memory` | Current contents of `memory.md` |
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
  "version": "0.7.0",
  "uptime_seconds": 123,
  "queue_depth_user": 0,
  "queue_depth_system": 0,
  "worker_threads": 1
}
```

#### `GET /memory`

**Response:** plain text containing the current `memory.md` contents.

## Session Lifecycle

1. **Creation** — If `session_id` is omitted, a new session is created with the current `Personality::system_prompt(memory_content)`.
2. **Validation** — If `session_id` is provided but unknown, `404` is returned immediately.
3. **Persistence** — The user message is appended via `ContextManager::add_user_message`.
4. **LLM call** — Messages are exported and sent through the `LlmWorkerPool`.
5. **Storage** — The assistant response is appended via `ContextManager::add_assistant_message`.

## Concurrency

Per-session requests are serialised using a `DashMap<String, Arc<Semaphore>>`. Cross-session requests run fully in parallel.

## CORS

`tower_http::cors::CorsLayer` is configured to allow:
- Origins: any (`*`)
- Methods: `GET`, `POST`
- Headers: `Content-Type`
