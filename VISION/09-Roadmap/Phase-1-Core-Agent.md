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
- [ ] `mimir stop` — signal daemon to shut down
- [ ] `mimir init` — first-run bootstrap (exists, needs systemd integration)

### 1.2 Mono-Binary Architecture
- [ ] Consolidate `mimir-cli` and `mimir-server` into a single `mimir` binary
- [ ] CLI commands (`ask`, `chat`, `status`, `memory`) talk to the daemon via HTTP, not directly to `mimir-core`
- [ ] `mimir start` runs the Axum server in-process (foreground, systemd manages backgrounding)
- [ ] Daemon-down detection: CLI prompts user to start the daemon if it is not running
- [ ] `mimir-server` becomes a library crate (no `main.rs`)
- [ ] New `mimir-client` library crate for HTTP client logic
- [ ] New `mimir` binary crate (single entry point, dispatches daemon vs client)

### 1.3 Chat Interface
- [x] Local HTTP server (Axum)
- [x] SSE for streaming responses
- [ ] Unix domain socket transport (see #25)
- [ ] Conversation history display
- [ ] Markdown rendering for responses

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
- [ ] `[server]` config section (bind_addr, socket_path placeholder)
- [ ] Hot-reload for non-sensitive config

### 1.9 Personality System
- [x] Personality presets (transparent, concise, warm, formal)
- [x] System prompt generation from memory.md content
- [x] CLI override via `--personality` flag

### 1.10 Deployment
- [ ] systemd user service file for `mimir start`
- [ ] `mimir init` offers to install and enable the systemd service
- [ ] Graceful shutdown via `mimir stop` (sends signal to daemon)

### 1.11 Testing
- [x] Unit tests for CLI parsing
- [x] Mock LLM client for testing
- [x] Integration tests for config and memory
- [ ] End-to-end test: CLI → daemon → response round-trip
- [ ] Unix socket transport tests (see #25)

## Architecture (Updated)

Mimir is a **single binary** that operates in two modes:

```
mimir (single binary)
├── Daemon mode (mimir start)
│   ├── Axum HTTP server (bind_addr + socket_path)
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
- Primary: Unix domain socket (`~/.local/share/mimir/mimir.sock`) — see #25
- Fallback: TCP localhost (`127.0.0.1:8080`) — for remote clients, web UI, Windows
- Daemon detection: check socket file existence (instant, no network)

## Success Criteria
- [x] `cargo build --workspace` succeeds
- [x] `cargo test --workspace` passes
- [ ] `mimir start` runs the daemon in the foreground (no separate binary)
- [ ] `mimir ask "hello"` talks to the daemon via HTTP
- [ ] `mimir chat` starts an interactive session via the daemon
- [ ] `mimir status` queries the daemon for health
- [ ] `mimir stop` signals the daemon to shut down
- [ ] Daemon-down prompt: CLI asks user if they want to start the daemon
- [ ] SSE streaming endpoint works for chat
- [ ] systemd user service works for auto-start

## Dependencies
- None (this is the foundation)

## Risks
- LLM API latency may make local testing slow
- Streaming SSE parsing edge cases
- Cross-platform config path differences
- Unix domain socket availability on Windows (graceful fallback to TCP)

## Related Issues
- #25 — Unix domain socket transport for local CLI↔daemon communication
