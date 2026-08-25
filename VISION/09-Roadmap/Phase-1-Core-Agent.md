# Phase 1: Core Agent

## Goal
Build the foundational interaction layer: CLI, chat interface, LLM orchestration, and basic tool-calling.

## Duration
4–6 weeks

## Deliverables

### 1.1 CLI Framework
- [x] `mimir start` — daemon launcher
- [x] `mimir ask "..."` — one-shot query
- [x] `mimir chat` — interactive REPL
- [x] `mimir status` — health overview
- [x] `mimir memory` — memory viewer
- [x] Command-line argument parsing (clap)
- [x] Structured logging (tracing)
- [x] `mimir stop` — signal daemon to shut down
- [x] `mimir init` — first-run bootstrap with optional systemd integration

### 1.2 Mono-Binary Architecture
- [x] Consolidate `mimir-cli` and `mimir-server` into a single `mimir` binary
- [x] CLI commands (`ask`, `chat`, `status`, `memory`) talk to the daemon via HTTP
- [x] `mimir start` runs the Axum server in-process (foreground, systemd manages backgrounding)
- [x] Daemon-down detection: CLI prompts user to start the daemon if it is not running
- [x] `mimir-server` becomes a library crate (no `main.rs`)
- [x] New `mimir-client` library crate for HTTP client logic
- [x] New `mimir` binary crate (single entry point, dispatches daemon vs client)

### 1.3 Chat Interface
- [x] Local HTTP server (Axum)
- [x] SSE for streaming responses
- [x] Unix domain socket transport (issue #25): the daemon serves the same router on a Unix socket alongside TCP; the local CLI prefers the socket
- [x] Conversation history display
- [x] Markdown rendering for responses

### 1.4 LLM Client
- [x] OpenAI-compatible HTTP client
- [x] Streaming support (SSE parsing)
- [x] Retry with exponential backoff
- [x] Token usage tracking
- [x] Configurable endpoint, model, temperature
- [x] Support for system prompts

### 1.5 Context Manager
- [x] In-memory conversation history (sliding window)
- [x] Session management
- [x] Context injection for multi-turn coherence

### 1.6 Tool Registry
- [x] Dynamic tool registration
- [x] JSON Schema generation for LLM function calling
- [x] Basic built-in tools: `get_current_time`, `echo`
- [x] CLI tool wrappers
- [x] Tool permission levels (Auto, Ask, Deny)

### 1.7 Skill Registry
- [x] Skill trait and registry
- [x] Built-in skills (research-synthesis, tdd)
- [x] User Markdown skill loading
- [x] System-generated skill scaffolding
- [x] Skill metrics tracking (SQLite)
- [x] CLI commands: list, show, add, delete, enable, disable

### 1.8 Configuration
- [x] TOML config file (`~/.config/mimir/config.toml`)
- [x] Environment variable overrides
- [x] XDG-aware path resolution (`paths` module)
- [x] Auto-initialisation of directories and defaults
- [x] `[server]` config section (bind_addr, socket_path placeholder)
- [x] Hot-reload for non-sensitive config

### 1.9 Personality System
- [x] Personality presets (transparent, concise, warm, formal)
- [x] System prompt composition from preset + shared operating directives + condensed knowledge-graph memory
- [x] CLI override via `--personality` flag

### 1.10 Deployment
- [x] systemd user service file for `mimir start`
- [x] `mimir init` offers to install and enable the systemd service
- [x] Graceful shutdown via `mimir stop` (POST `/stop` to daemon)

### 1.11 Testing
- [x] Unit tests for CLI parsing
- [x] Mock LLM client for testing
- [x] Integration tests for config and memory
- [x] End-to-end test: CLI → daemon → response round-trip
- [x] Unix socket transport tests (issue #25): full-daemon `/health` and `/stop` round trips over the socket, socket cleanup on shutdown, stale-socket recovery

## Architecture (Updated)

Mimir is a **single binary** that operates in two modes:

```
mimir (single binary)
├── Daemon mode (mimir start)
│   ├── Axum HTTP server (TCP localhost)
│   ├── LlmWorkerPool (shared across all requests)
│   ├── ContextManager (shared across all sessions)
│   ├── ToolRegistry + SkillRegistry
│   └── Future: connectors, proactive agent, reasoning engine
└── Client mode (mimir ask, chat, status, memory, stop)
    └── mimir-client (HTTP client → daemon)
```

Library crates provide code organisation:
- `mimir-core` — LLM client, config, memory, context, personality, tools, skills, paths
- `mimir-server` — Axum routes, state, middleware (library, no binary)
- `mimir-client` — HTTP client for talking to the daemon
- `mimir` — binary crate (dispatches daemon vs client)

### Transport
- **Unix domain socket** (`~/.local/share/mimir/mimir.sock`) — preferred local transport (issue #25); instant daemon detection via a local socket connection
- **TCP** (`127.0.0.1:8080`) — fallback for remote clients (`MIMIR_BASE_URL`) and Windows
- Daemon detection: socket-file existence on Unix, TCP health probe (`GET /health`) otherwise, with fallback auto-start prompt

## Success Criteria
- [x] `cargo build --workspace` succeeds
- [x] `cargo test --workspace` passes
- [x] `mimir start` runs the daemon in the foreground (no separate binary)
- [x] `mimir ask "hello"` talks to the daemon via HTTP
- [x] `mimir chat` starts an interactive session via the daemon
- [x] `mimir status` queries the daemon for health
- [x] `mimir stop` signals the daemon to shut down
- [x] Daemon-down prompt: CLI asks user if they want to start the daemon
- [x] SSE streaming endpoint works for chat
- [x] systemd user service works for auto-start
- [x] `mimir-client` crate exists and is a workspace member
- [x] Conversation history display works in `mimir chat`
- [x] Markdown responses are preserved in terminal output
- [x] End-to-end round-trip test passes
- [x] Config hot-reload works for non-sensitive settings

## Dependencies
- None (this is the foundation)

## Risks
- LLM API latency may make local testing slow
- Streaming SSE parsing edge cases
- Cross-platform config path differences
- Unix domain socket availability on Windows (graceful fallback to TCP) — resolved: non-Unix platforms use TCP only

## Related Issues
- #30 — Create `mimir-client` crate and migrate CLI to HTTP client mode
- #31 — `mimir stop` command and graceful daemon shutdown
- #32 — Config hot-reload for non-sensitive settings
- #33 — Daemon-down detection with TCP health check and auto-start prompt
- #34 — systemd user service integration
- #35 — End-to-end CLI → daemon → response round-trip test
- #36 — Conversation history display and markdown rendering in chat
- #12 — Phase 1 Finalize Workspace (parent tracking issue)
- #25 — Unix domain socket transport for local CLI↔daemon communication (implemented)
