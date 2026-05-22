# Server

Mimir includes an HTTP server (`mimir-server`) that exposes chat, status, and memory endpoints.

## Starting the Server

Run the server binary directly:

```bash
cargo run -p mimir-server
```

By default it binds to `127.0.0.1:8080`.

## Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/status` | `GET` | Health check and runtime statistics |
| `/memory` | `GET` | Current `memory.md` contents |
| `/chat` | `POST` | Blocking chat completion |
| `/chat/stream` | `POST` | SSE streaming chat completion |

### Status

```bash
curl http://127.0.0.1:8080/status
```

Returns version, uptime, and queue depths.

### Memory

```bash
curl http://127.0.0.1:8080/memory
```

Returns the raw contents of `memory.md`.

### Chat

See [Chat API](chat-api.md) for detailed examples.

## Architecture

- Built on [Axum](https://github.com/tokio-rs/axum).
- LLM requests are routed through a worker pool with user and system priority queues.
- Sessions are persisted in SQLite via `ContextManager`.
- Per-session concurrency is controlled via semaphores.
