# Server

Mimir includes an HTTP server that exposes chat, status, and memory endpoints. The server runs in-process as part of the `mimir` binary.

## Starting the Server

```bash
mimir start
```

This runs the Axum HTTP server in the foreground. For production use, systemd manages backgrounding and restarts.

By default, it binds to `127.0.0.1:8080`. Configure this in `~/.config/mimir/config.toml`:

```toml
[server]
bind_addr = "127.0.0.1:8080"
# socket_path = "~/.local/share/mimir/mimir.sock"  # Optional: Unix domain socket
```

> **CLI target follows `bind_addr`.** Client commands (`ask`, `chat`, `status`, …) automatically target the daemon's configured `bind_addr` (with wildcard hosts like `0.0.0.0` normalised to loopback), overridden by the `MIMIR_BASE_URL` environment variable. So changing `bind_addr` no longer requires also setting `MIMIR_BASE_URL` to keep the CLI working.


## Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | `GET` | Cheap liveness probe used by the daemon guard (no LLM/DB work) |
| `/status` | `GET` | Health check and runtime statistics |
| `/memory` | `GET` | Live condensed memory block from the knowledge graph |
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

Returns the live condensed memory block (stable facts + upcoming events) from the knowledge graph.

### Chat

See [Chat API](chat-api.md) for detailed examples.

## Architecture

- Built on [Axum](https://github.com/tokio-rs/axum).
- LLM requests are routed through a worker pool with user and system priority queues.
- Sessions are persisted in SQLite via `ContextManager`.
- Per-session concurrency is controlled via semaphores.
- The server is a library crate (`mimir-server`); the `mimir` binary calls `mimir_server::build_app()` and `mimir_server::start_server()`.

## Stopping the Server

Send `SIGINT` or `SIGTERM` to the process (e.g., `Ctrl+C` in the foreground, or `systemctl --user stop mimir` when running as a systemd service).
