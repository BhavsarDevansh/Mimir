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
| `GET` | `/v1/models` | OpenAI-compatible model list (personality presets) |
| `POST` | `/v1/chat/completions` | OpenAI-compatible chat completion (blocking + streaming) |

## Authentication

Every route except `GET /health` requires a bearer token (issue #281): requests must present `Authorization: Bearer <token>` or they are rejected with `401 Unauthorized` and a `WWW-Authenticate: Bearer` challenge. The token is generated automatically at `~/.local/share/mimir/api_token` (mode `0600`) and the `mimir` CLI attaches it to every request, so CLI commands work unmodified. `GET /health` stays unauthenticated because it is the daemon-guard liveness probe. See `docs/api-authentication.md` for the threat model, token lifecycle, and non-loopback bind guidance.

## Loopback Guard

Destructive and sensitive routes are additionally loopback-only: the `require_loopback` middleware in `mimir-server/src/app.rs` rejects non-loopback callers with `403 Forbidden` before the handler runs, so a daemon bound to a LAN address never accepts remote mutations even from a caller that holds the token. Gated routes: `POST /memory/refresh`, `POST /kb/optimization/run-now`, `POST /kb/facts/forget`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject`, `GET /kb/pending`, `DELETE /kb/trash`, `POST /kb/trash/restore`, `POST /connectors/{id}/tokens`, `POST /connectors/{id}/forget`, and `POST /stop`. The auth layer runs outside the loopback layer, so an unauthenticated non-loopback caller gets `401` before the `403` loopback check.

### Request/Response Schemas

#### `POST /chat`

**Request body:**
```json
{
  "session_id": 42,
  "message": "Hello, Mimir!"
}
```

**Response body (success):**

```json
{
  "session_id": 42,
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
- `event: error\ndata: …` — on mid-stream failure (terminal). The data carries the flattened, bounded failure message (e.g. an upstream LLM `503` overload) instead of a generic `internal server error`, so clients see the actionable cause.

Keep-alive pings are sent every 10 seconds.

#### `GET /status`

**Response body:**
```json
{
  "version": "0.11.0",
  "uptime_seconds": 123,
  "queue_depth_user": 0,
  "queue_depth_system": 0,
  "hook_queue_depth": 0,
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
    "session_id": 42,
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
  "session_id": 42,
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
5. **Tool Calls** — If the LLM responds with `tool_calls` (OpenAI function-calling format), each tool is executed via `ToolRegistry`, the results are appended as `role: tool` messages, and a follow-up LLM request is made to obtain the final assistant text. Both the blocking (`/chat`) and streaming (`/chat/stream`) endpoints support this loop. In streaming mode, tool-call deltas are accumulated internally and the final text is streamed after execution. Every tool — including `retrieve_context` — dispatches through the registry with a per-request `ToolContext` carrying the request-resolved LLM and the incognito write-tool policy, so permission checks apply uniformly in one place (issue #441).
6. **Storage** — The final assistant response is appended via `ContextManager::add_assistant_message`.
7. **Fact Extraction** — For non-incognito sessions, the route triggers the `remember.chat` hook (`Trigger::TurnCompleted`) after the assistant response is persisted. The hook engine debounces consecutive turns per session and, once the LLM pool is idle, runs the fact-extraction pipeline (`KnowledgeGraph::extract_facts_with_context`) over the accumulated transcript. Sensitive facts are gated pending user confirmation.

## Incognito Mode (issue #155)

When a chat request sets `incognito: true`:

- No session is created and neither the user message nor the assistant response is persisted.
- **Write-capable tools are suppressed.** Tools implementing `Tool::is_write_tool() -> true` are excluded from the exported tool set, and any attempt to execute them during an incognito turn returns `ToolError::BlockedIncognito` so no facts are written to the knowledge graph. No built-in tool is currently write-capable (the `remember` tool was removed in #386), but the guard remains as defence-in-depth. Read-only KG tools remain available.
- **No hooks fire.** Incognito turns never enqueue the `remember.chat` hook, so no background extraction runs and no facts are persisted (asserted by server integration tests).
- The live configuration temperature is applied per request via `LlmBackend::with_temperature_override` (issue #80), so hot-reloaded `llm.temperature` changes take effect without restarting the daemon. The override clone keeps the worker pool (issue #465), so incognito and persisted turns alike still enqueue on the user queue and queue-full backpressure applies.

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

`mimir-server` splits the daemon into `app.rs` (router assembly + bearer-token auth + loopback guard), `server.rs` (startup and background tasks), `shutdown.rs` (signal handling + bounded graceful drain), `state/` (shared `AppState` construction), `routes/` (one module per endpoint family — `chat.rs`, `connectors.rs`, `kb/`, `memory.rs`, `sessions.rs`, `status.rs`, `stop.rs`, `kb_categories.rs`), and `error.rs` (wire error mapping). The KB route family is further split by concern in `routes/kb/` (`query`, `detail`, `browse`, `pending`, `trash`, `forget`, `optimization`, `helpers`, `params`).

`AppState` construction (`state/builder.rs`) is decomposed into per-subsystem init helpers composed by `from_config_with_llm` in a fixed startup order: `init_context_manager` → `init_tool_registry` → `init_knowledge_graph` (geocoder injection, user-entity resolution, identity-fact seeding, KG tool registration) → `init_job_queue` → `init_agent_runtime` → `init_scheduler` (registers the knowledge-optimization, pending-cleanup, and events-scan jobs) → `init_hook_engine` (registers the `remember.chat`, `memory.condensation`, and `connector_item.remember` hooks) → `init_connector_framework` (feature-gated factory registration, supervisor wiring, `restore()`). Each helper is independently unit-testable; issue #281 added the `api_token` field to `AppState` (loaded or generated at startup).

## OpenAI-Compatible Provider Surface

The daemon also exposes an OpenAI-compatible provider surface (`GET /v1/models` and `POST /v1/chat/completions`) so apps and devices that speak the OpenAI chat-completions API can use Mimir as their LLM provider. The OpenAI `user` field is a conversation key that resumes one persistent session in the central profile; requests without `user` (or with a blank one) key the fixed `default` session, so every request is persisted and every completed turn fires the learning hooks — there is no incognito path on this surface (issue #473). Model names matching a personality preset select that preset, and unknown names pass through as upstream model overrides. Client tool schemas are merged with Mimir's server-side tools (server wins on name collision), and `/v1` errors use the OpenAI error JSON shape. See `docs/llm-provider.md` for the full design and `docs/wiki/llm-provider.md` for usage.
