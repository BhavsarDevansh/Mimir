# Mimir — Implementation Context

> **Created:** 2025-05-20
> **Last Updated:** 2026-06-25
> **Vision Docs:** `VISION/` directory (48 files, 10 sections)
> **Phase 1 Plan:** `VISION/09-Roadmap/Phase-1-Core-Agent.md`
> **GitHub:** https://github.com/BhavsarDevansh/Mimir

---

## What Is Mimir?

Mimir is a persistent, personal intelligence that learns from your life, connects to your services, and becomes more useful the longer you use it. It is NOT a chatbot — it is a stateful, ever-learning companion.

**Name origin:** Mimir, the Norse god of wisdom whose severed head preserved all knowledge and gave counsel to the gods.

**License:** GNU GPL-3.0

---

## Core Principles

1. **Persistence over ephemerality** — Every interaction, fact, and preference is stored, versioned, and retrievable
2. **Implicit learning** — The agent observes, generalizes, and adjusts without explicit training
3. **User sovereignty** — Inspect, edit, and delete anything. The knowledge base is yours
4. **Thoroughness** — Investigates all available avenues, not just the first plausible answer
5. **Proactivity** — Earns trust, then anticipates needs rather than only responding
6. **Openness** — OpenAI-compatible API endpoint; pluggable connectors for services
7. **Local-first** — All data stays on your device. No cloud intermediary

---

## Architecture Overview

### Single Binary, Two Modes

Mimir is distributed as a single `mimir` binary that operates in two modes:

```
mimir (single binary)
├── Daemon mode (mimir start)
│   ├── Axum HTTP server (bind_addr + socket_path)
│   ├── LlmWorkerPool (shared across all requests)
│   ├── ContextManager (shared across all sessions)
│   ├── ToolRegistry + SkillRegistry
│   ├── MemoryManager + MemoryLoader
│   └── Future: connectors, proactive agent, reasoning engine
└── Client mode (mimir ask, chat, status, memory, stop)
    └── mimir-client (HTTP client → daemon)
```

### Library Crates (code organisation, not separate binaries)

| Crate | Type | Role |
|-------|------|------|
| `mimir-core` | library | LLM client, config, memory, context, personality, tools, skills, paths |
| `mimir-server` | library | Axum routes, state, middleware |
| `mimir-client` | library | HTTP client for talking to the daemon |
| `mimir` | binary | Single entry point — dispatches daemon or client mode |

### Transport

The daemon exposes its API over two transports simultaneously:
1. **Unix domain socket** (`~/.local/share/mimir/mimir.sock`) — planned for local CLI (see #25; not yet implemented)
2. **TCP** (`127.0.0.1:8080`) — fallback for remote clients, web UI, and Windows

### Daemon-down Handling

When a CLI command cannot connect to the daemon, the user is prompted:
```
Error: Mimir is not running.
Start the server now? [y/N]:
```
If the user agrees, the daemon is started and the command is retried.

### System Diagram

```
┌─────────────────────────────────────────────────────────┐
│                     User Interfaces                       │
│  (CLI, Chat UI, WebSocket, Proactive Notifications)    │
└─────────────────────────────────────────────────────────┘
                           │
                    Unix socket / TCP
                           │
┌─────────────────────────────────────────────────────────┐
│                     Core Agent (Daemon)                  │
│  (Input Router, Context Manager, Response Synthesizer) │
└─────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────┐
│              Subsystems (all in Rust)                    │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │  Reasoning   │  │  Knowledge   │  │  Proactive   │  │
│  │   Engine     │  │    Graph     │  │    Agent     │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │  Connectors  │  │   memory.md  │  │    Vision    │  │
│  │  Framework   │  │ (Working    │  │  Tracking    │  │
│  └──────────────┘  │   Memory)    │  └──────────────┘  │
│                     └──────────────┘                     │
└─────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────┐
│                     Storage Layer                        │
│         SQLite (local-first, single file)                │
│  - Knowledge Graph (entities, facts, temporal data)     │
│  - memory.md (hot cache, 2,500 char limit)              │
│  - Audit logs, patterns, preferences                    │
└─────────────────────────────────────────────────────────┘
```

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust (edition 2024) |
| Async Runtime | tokio |
| HTTP Server | axum |
| HTTP Client | reqwest |
| Database | SQLite (sqlx) |
| Config | TOML (serde + toml) |
| CLI | clap |
| Serialization | serde |
| Logging | tracing |
| LLM API | OpenAI-compatible (any provider) |

---

## Key Design Decisions

### Single Binary, Library Crates

The workspace produces one binary (`mimir`) but uses library crates for code organisation:
- `mimir-core` — shared domain logic (used by both daemon and tests)
- `mimir-server` — HTTP API layer (library, no binary)
- `mimir-client` — HTTP client for CLI commands (library, no binary)
- `mimir` — binary crate, thin dispatcher

This avoids the problems of a two-binary architecture (separate `mimir` CLI and `mimir-server`):
- No fragile process spawning (old `mimir start` searched PATH for a second binary)
- No duplicated state (each binary had its own LlmClient, ContextManager, etc.)
- No coordination gap (separate processes with no shared state)
- Single systemd unit, single binary to install

### Daemon as the Single Source of Truth

All state lives in the daemon process. CLI commands are thin HTTP clients:
- `mimir ask` → `POST /chat` or `POST /chat/stream`
- `mimir chat` → interactive SSE client
- `mimir status` → `GET /status`
- `mimir memory` → `GET /memory`
- `mimir stop` → `POST /shutdown` (or SIGTERM via systemd)

### Unix Domain Socket Transport

The daemon will listen on both a Unix domain socket and a TCP socket once UDS is implemented (see #25). Currently, only TCP localhost (`127.0.0.1:8080`) is active. The CLI will prefer the Unix socket (faster, more secure, instant daemon detection) and fall back to TCP (for remote clients, web UI, Windows) when UDS is available.

### systemd Integration

`mimir start` runs the daemon in the foreground. systemd manages backgrounding, restart-on-failure, and logging. `mimir init` offers to install the systemd user service. See `VISION/08-Architecture/Deployment-Model.md`.

---

## Personality System

- **Presets:** `transparent` (default), `concise`, `warm`, `formal`
- **System prompt:** Generated from personality preset + memory.md content
- **Override:** `mimir ask -p concise "..."` or `mimir chat` then `/personality concise`
- **Extensible:** Custom personalities can be added via config in future versions

---

## Memory System

### memory.md (Working Memory)
- ~2,500 character budget (~900 tokens)
- Injected into every system prompt for fast context
- Auto-managed: add, replace, remove entries
- Frozen per session (snapshot taken at start)
- Persisted to disk immediately on change

### Context Manager
- SQLite-backed session and message storage
- Sliding window of recent conversation
- Token-aware trimming (removes oldest pairs first)
- Cumulative token usage tracking per session

---

## Tool & Skill System

### Tools
- Object-safe `Tool` trait: `name()`, `description()`, `parameters_schema()`, `execute()`
- Three permission levels: `Auto` (always run), `Ask` (confirm first), `Deny` (never run)
- Built-in: `echo`, `get_current_time`
- CLI wrappers: invoke external commands as tools

### Skills
- Object-safe `Skill` trait with `SkillContext` (access to tools, LLM, context)
- Three sources: `Builtin`, `User` (Markdown files), `Generated` (auto-created)
- Metrics tracked in SQLite: invocation count, success rate, user corrections
- Phase B (issue #20): system-generated skills, utility scoring, pruning, promotion

---

## Proactivity System (Phase 5)

- **Trust ladder:** Observation → Gentle Offers → Pattern Permissions → Autonomous
- **Proactivity levels:** `never`, `important_only`, `always`
- **Notification fatigue detection:** If 3+ dismissals in a row, pause proactivity.

---

## Knowledge Graph (Phase 2)

- SQLite-based, single file, local-first.
- Entities, Facts (directed temporal edges), Sources (provenance), Preferences.
- Temporal facts: `valid_from`, `valid_until`. History is preserved.
- Confidence scores: 0.0-1.0. Facts color-coded by confidence.
- Obsidian-compatible export/import (Markdown + YAML frontmatter + wiki-links).
- Nightly optimization: deduplication, contradiction resolution, dormant cleanup.
- **Events & reminders (#74, v0.57.0):** a lifecycle + recurrence overlay on facts. A future-dated fact is a one-time event; a recurring fact (e.g. a birthday) is a recurring event; a `requires_user_action` fact is a task. An `events.upcoming_scan` job (default 06:00 & 18:00) derives overlays, auto-completes past one-time events, and advances recurring events. Upcoming events surface in the "Upcoming" memory section. `entity_dates` is deprecated and removed (replaced by this overlay; recurrence logic moved to `models::recurrence`).

---

## Phase 1: Core Agent (Current Plan)

**Goal:** Build the foundational layer. The agent can start, hold a conversation, stream responses from an OpenAI-compatible endpoint, and manage memory.md.

**Key deliverables:**
- Single `mimir` binary with daemon and client modes
- CLI commands talk to daemon via HTTP (Unix socket preferred, TCP fallback)
- Daemon-down detection with user prompt to start
- systemd integration for auto-start
- LLM streaming, context management, personality presets
- Tool and skill registries

See `VISION/09-Roadmap/Phase-1-Core-Agent.md` for full task list.

---

## Phase 2+: Roadmap Summary

| Phase | Focus | Duration | Key Deliverables |
|-------|-------|----------|-----------------|
| 1 | Core Agent | 4-6 weeks | Single binary, daemon/client, CLI, chat, LLM, memory.md |
| 2 | Knowledge Graph | 4-6 weeks | SQLite schema, entities, facts, temporal queries |
| 3 | Connectors | 6-8 weeks | Gmail, Calendar, Photos, normalization pipeline |
| 4 | Reasoning Engine | 6-8 weeks | Multi-thread investigation, meta-threads, streaming |
| 5 | Proactive Agent | 4-6 weeks | Event monitoring, pattern recognition, trust ladder |
| 6 | Vision Tracking | 6-8 weeks | Object detection, spatial memory, re-identification |

---

## Important Files and Locations

### Config
- User config: `~/.config/mimir/config.toml`
- Default config: `config/default.toml`
- memory.md: `~/.config/mimir/memory.md`
- Data: `~/.local/share/mimir/`
- Socket: `~/.local/share/mimir/mimir.sock`

### Key VISION Docs (if you need to reference)
- `VISION/00-Overview/Vision-Statement.md` — Core premise and principles
- `VISION/01-Core-Agent/Personality.md` — Personality system
- `VISION/01-Core-Agent/Memory-System.md` — memory.md design
- `VISION/01-Core-Agent/Technical-Design.md` — Architecture, single binary design
- `VISION/01-Core-Agent/User-Experience.md` — CLI and daemon interaction
- `VISION/02-Knowledge-Graph/Learning-Modes.md` — Explicit vs casual learning
- `VISION/02-Knowledge-Graph/Temporal-Facts.md` — Temporal storage model
- `VISION/04-Reasoning-Engine/Technical-Design.md` — Investigation threads, meta-threads
- `VISION/05-Proactive-Agent/User-Experience.md` — Trust ladder
- `VISION/08-Architecture/Deployment-Model.md` — systemd, single binary, Unix socket
- `VISION/08-Architecture/Permission-Model.md` — Permission levels

---

## How to Start Implementing

1. **Clone the repo:** `git clone https://github.com/BhavsarDevansh/Mimir.git`
2. **Read the Phase 1 roadmap:** `VISION/09-Roadmap/Phase-1-Core-Agent.md`
3. **Start with the mono-binary consolidation** (current work)
4. **TDD throughout** — every feature starts with a failing test

---

## Environment Prerequisites

- Rust toolchain (latest stable, edition 2024)
- SQLite development libraries
- An OpenAI-compatible API key (or local model endpoint)
- Git

---

## Success Criteria for Phase 1

- [x] `cargo build --workspace` succeeds
- [x] `cargo test --workspace` passes
- [x] `mimir start` runs the daemon in the foreground (no separate binary)
- [ ] `mimir ask "hello"` talks to the daemon via HTTP (tracked in #30)
- [ ] `mimir chat` starts an interactive session via the daemon (tracked in #30)
- [ ] `mimir status` queries the daemon for health (tracked in #30)
- [ ] `mimir stop` signals the daemon to shut down (tracked in #31)
- [ ] Daemon-down prompt: CLI asks user if they want to start the daemon (tracked in #33)
- [x] SSE streaming endpoint works for chat
- [ ] systemd user service works for auto-start (tracked in #34)
- [ ] `mimir-client` crate exists and is a workspace member (tracked in #30)
- [ ] Conversation history display works in `mimir chat` (tracked in #36)
- [ ] Markdown responses are preserved in terminal output (tracked in #36)
- [ ] End-to-end round-trip test passes (tracked in #35)
- [ ] Config hot-reload works for non-sensitive settings (tracked in #32)

---

*End of Implementation Context*
