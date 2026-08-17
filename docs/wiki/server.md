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
| `/connectors` | `GET` | List registered connector instances with derived item counts |
| `/connectors` | `POST` | Register a new connector instance (add-only) |
| `/connectors/{id}` | `GET` | Show a single connector instance with its item count |
| `/connectors/{id}` | `DELETE` | Stop the runner and delete the instance |

### Status

`/status` requires the API token; `GET /health` is the only unauthenticated route (see [Authentication](#authentication)).

```bash
TOKEN=$(cat ~/.local/share/mimir/api_token)
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/status
```

Returns version, uptime, and queue depths.

### Memory

`/memory` requires the API token; `GET /health` is the only unauthenticated route (see [Authentication](#authentication)).

```bash
TOKEN=$(cat ~/.local/share/mimir/api_token)
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/memory
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

## Authentication

Every request except `GET /health` must present the daemon's API token as `Authorization: Bearer <token>`, otherwise the daemon answers `401 Unauthorized`. The token is generated automatically at `~/.local/share/mimir/api_token` (mode `0600`) during `mimir init` — or lazily by the daemon or any CLI command — and the CLI attaches it to every request, so `mimir ask`, `mimir chat`, `mimir kb`, and the other commands keep working without any extra setup. If the CLI cannot load or attach the token it prints a warning and continues, so the daemon's `401` response surfaces the problem. `GET /health` stays open because it is the lightweight "is the daemon running?" probe. If you bind the daemon to a non-loopback address such as `0.0.0.0`, the token is the only thing protecting your data, so treat the token file like a password and rotate it (delete the file and restart the daemon) if it leaks. See [API Authentication](../api-authentication.md) for the full picture.

## Loopback-Only Routes

Destructive and sensitive operations are additionally only accepted from the local machine. If the daemon is bound to a LAN address, remote clients that hold the token can still read status and KB queries and use chat — note that chat persists turns and can write facts through the `remember` tool — but mutations such as forgetting facts, emptying the trash, restoring facts, triggering optimization, ingesting connector credentials, and stopping the daemon return `403 Forbidden` for non-loopback callers. This keeps the single-writer knowledge graph safe even when the server is reachable from other devices.

## Stopping the Server

Send `SIGINT` or `SIGTERM` to the process (e.g., `Ctrl+C` in the foreground, or `systemctl --user stop mimir` when running as a systemd service).
