# Deployment Model

## Local-First Desktop Deployment

### Primary Target
A single-user daemon running on the user's primary machine (desktop/laptop).

**Process Model:**
```
mimir (single binary, daemon mode)
  ├── HTTP server (Axum, bind_addr + socket_path)
  ├── LlmWorkerPool (shared across all requests)
  ├── ContextManager (shared across all sessions)
  ├── ToolRegistry + SkillRegistry
  ├── connector-manager (spawns connector tasks, future)
  ├── reasoning-engine (on-demand, future)
  ├── proactive-agent (background scheduler, future)
  ├── vision-tracker (optional, future)
  └── knowledge-graph (SQLite, shared across all)
```

The same binary runs in client mode for CLI commands:
```
mimir ask/chat/status/memory/stop → HTTP client → daemon
```

**Single Binary Architecture:**

Mimir is distributed as a single `mimir` binary that operates in two modes:
- **Daemon mode** (`mimir start`): runs the persistent HTTP server with all subsystems
- **Client mode** (`mimir ask`, `mimir chat`, etc.): thin HTTP client that talks to the daemon

Library crates provide code organisation but produce one binary:
- `mimir-core` — LLM client, config, memory, context, personality, tools, skills, paths
- `mimir-server` — Axum routes, state, middleware (library, no binary)
- `mimir-client` — HTTP client for talking to the daemon
- `mimir` — binary crate (dispatches daemon vs client)

**Transport:**
- **Active:** TCP localhost (`127.0.0.1:8080`) — used for all clients (local and remote)
- **Planned (#25):** Unix domain socket (`~/.local/share/mimir/mimir.sock`) — will offer faster local IPC, instant daemon detection, and filesystem-level access control
- Planned: Daemon detection — check socket file existence (instant, no network; not yet implemented, tracked as `#25`)

**API authentication (issue #281):** every route except `GET /health` requires a bearer token auto-generated at `~/.local/share/mimir/api_token` (mode `0600`); the CLI attaches it automatically. A non-loopback bind (e.g. `0.0.0.0:8080`) is protected only by this token — treat the token file like a password and prefer a reverse proxy with TLS and its own authentication for LAN exposure.

**Daemon-down handling:** When a CLI command cannot reach the daemon, the user is prompted:
```
Error: Mimir is not running.
Start the server now? [y/N]:
```
If the user agrees, the daemon is started in-process and the command is retried.

**Storage:**
- Config: `~/.config/mimir/`
- Data: `~/.local/share/mimir/`
- Cache: `~/.cache/mimir/`
- Logs: `~/.local/share/mimir/logs/`
- Socket: `~/.local/share/mimir/mimir.sock`

### Platform Support
- **Linux:** Primary development target (systemd user service)
- **macOS:** Supported (launchd user agent)
- **Windows:** Supported (TCP-only, no Unix socket)

### systemd Integration

```ini
[Unit]
Description=Mimir — persistent personal intelligence
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/mimir start
Restart=on-failure
RestartSec=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/.local/share/mimir %h/.config/mimir
PrivateTmp=true

# Logging → journalctl --user -u mimir
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
```

Enable with:
```bash
systemctl --user daemon-reload
systemctl --user enable --now mimir
loginctl enable-linger $USER   # runs even when not logged in
```

## Self-Hosted Server Deployment

For users who want the agent to run 24/7 on a home server or NAS.

**Configuration:**
- All connectors run server-side
- Web UI accessible from local network
- Optional: reverse proxy with authentication
- SQLite sufficient for single-user; optional PostgreSQL for scale

## Container Deployment

```dockerfile
FROM rust:1.85-slim AS builder
# ... build mimir

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y sqlite3 ffmpeg
COPY --from=builder /app/target/release/mimir /usr/local/bin/mimir
ENTRYPOINT ["mimir", "start"]
```

```yaml
# docker-compose.yml
version: '3'
services:
  mimir:
    image: mimir:latest
    volumes:
      - ./data:/home/mimir/.local/share/mimir
      - ./config:/home/mimir/.config/mimir
    environment:
      - MIMIR_LLM_API_KEY=sk-...
    ports:
      - "127.0.0.1:8080:8080"
    restart: unless-stopped
```

## Scaling Considerations

### Single User, Large Graph
- SQLite handles millions of facts comfortably
- If graph grows >10M facts: migrate to PostgreSQL or Oxigraph
- Vector embeddings: local FAISS or pgvector

### Multi-User (Future)
- Row-level security in PostgreSQL
- Per-user encryption keys
- Shared connectors with scoped permissions
- Not in initial scope

## Hardware Requirements

### Minimum
- CPU: 2 cores (x86_64 or ARM64)
- RAM: 2 GB
- Storage: 1 GB (grows with connectors and history)
- Network: Broadband for LLM API and connector syncs

### Recommended
- CPU: 4+ cores
- RAM: 8 GB
- Storage: SSD, 10+ GB
- GPU: Optional, for local vision model acceleration

### Without GPU
- Object detection: YOLO on CPU (slower but functional)
- Embeddings: ONNX Runtime CPU
- LLM: Always remote (OpenAI API)
