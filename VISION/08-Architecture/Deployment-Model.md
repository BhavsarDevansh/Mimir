# Deployment Model

## Local-First Desktop Deployment

### Primary Target
A single-user daemon running on the user's primary machine (desktop/laptop).

**Process Model:**
```
agent-daemon (main process)
  ├── core-agent (HTTP API + CLI handler)
  ├── connector-manager (spawns connector tasks)
  ├── reasoning-engine (on-demand)
  ├── proactive-agent (background scheduler)
  ├── vision-tracker (optional, if cameras configured)
  └── knowledge-graph (SQLite, shared across all)
```

**Storage:**
- Config: `~/.config/agent/`
- Data: `~/.local/share/agent/`
- Cache: `~/.cache/agent/`
- Logs: `~/.local/share/agent/logs/`

### Platform Support
- **Linux:** Primary development target (systemd user service)
- **macOS:** Supported (launchd user agent)
- **Windows:** Supported (Windows Service or background process)

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
# ... build agent

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y sqlite3 ffmpeg
COPY --from=builder /app/target/release/agent /usr/local/bin/agent
ENTRYPOINT ["agent", "start"]
```

```yaml
# docker-compose.yml
version: '3'
services:
  agent:
    image: agent:latest
    volumes:
      - ./data:/data
      - ./config:/config
    environment:
      - AGENT_CONFIG=/config/agent.toml
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
