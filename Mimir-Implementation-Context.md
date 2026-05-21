# Mimir — Implementation Context

> **Created:** 2025-05-20
> **Last Updated:** 2025-05-20
> **Vision Docs:** `VISION/` directory (48 files, 10 sections)
> **Phase 1 Plan:** `docs/superpowers/plans/2025-05-20-mimir-phase-1-core-agent.md`
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

```
┌─────────────────────────────────────────────────────────┐
│                     User Interfaces                       │
│  (CLI, Chat UI, WebSocket, Proactive Notifications)    │
└─────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────┐
│                     Core Agent                           │
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
| Database | SQLite (rusqlite / sqlx) |
| Config | TOML |
| CLI | clap |
| Serialization | serde |
| Logging | tracing |
| File Watching | notify |
| LLM API | OpenAI-compatible (any provider) |

---

## Workspace Structure (Phase 1+)

```
/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── mimir-cli/                # CLI binary (`mimir` command)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs
│   ├── mimir-core/               # Core library (config, LLM, memory, context, tools, personality)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs
│   │       ├── llm/
│   │       │   ├── mod.rs
│   │       │   ├── client.rs     # HTTP client + SSE streaming
│   │       │   └── types.rs      # Request/response types
│   │       ├── memory/
│   │       │   ├── mod.rs
│   │       │   ├── loader.rs     # File loading + hot-reload
│   │       │   └── manager.rs    # Auto-management (add/replace/remove)
│   │       ├── context.rs        # Conversation context (sliding window)
│   │       ├── tools/
│   │       │   ├── mod.rs
│   │       │   └── registry.rs   # Dynamic tool registration
│   │       └── personality.rs    # Personality presets + system prompt
│   └── mimir-server/             # HTTP API daemon
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           └── routes/
│               ├── mod.rs
│               └── chat.rs       # SSE streaming endpoint
├── config/
│   └── default.toml              # Default configuration
├── VISION/                       # 48 documentation files
├── docs/superpowers/plans/       # Implementation plans
└── tests/
    └── integration_tests.rs
```

---

## Key Design Decisions

### 1. memory.md — Working Memory System
- A 2,500-character (~900 token) "hot cache" injected into every system prompt
- Auto-managed by the agent using `add`, `replace`, `remove` tools
- Contains: Identity, Active Projects, Preferences, Temporal Facts, KB Pointers
- Frozen snapshot per session (preserves LLM prefix cache)
- File: `~/.config/mimir/memory.md`
- When full, agent consolidates entries before adding new ones

### 2. Trust Ladder (Permission Model)
- **Level 0 (Observation):** Agent reads but never acts. Default for new connectors.
- **Level 1 (Ask Every Time):** Agent detects opportunities and asks explicitly.
- **Level 2 (Category Permission):** User grants permission for a class of actions.
- **Level 3 (Domain Permission):** User grants permission for a whole domain.
- **Level 4 (Full Authority):** Agent acts autonomously for low-risk actions.
- Permissions are stored as high-confidence preferences in the Knowledge Graph.
- Permissions are revocable at any time via natural language or CLI.

### 3. Reasoning Engine — Investigation Model
- **Every query is an investigation.** Direct tool hits answer immediately.
- If ambiguous or no direct tool: launch up to 5 parallel investigation threads.
- Each thread can drill down up to 3 levels deep with sub-threads.
- **Meta-thread:** Spawned when threads contradict. Investigates WHY and reconciles.
- **Real-time streaming:** User watches investigation unfold live.
- **Stopping criteria:** Direct hit, consensus, exhaustion, time budget, user interrupt.

### 4. Learning Modes
- **Explicit statements (confidence 1.0):** User directly asserts a fact. Overwrites existing facts.
- **Casual mentions (confidence 0.2-0.4):** Passing reference. Does NOT overwrite.
- **Connector extraction (confidence varies):** Does not overwrite explicit facts.
- **Reasoning inference (confidence 0.3-0.6):** Never overwrites anything.
- Sensitive facts (health, financial) require explicit confirmation before storage.

### 5. Personality System
- **Default: Transparent** — Shows work, admits uncertainty, asks clarifying questions.
- **Presets:** Transparent, Concise, Warm, Formal.
- **Custom:** User-defined system prompt via `personality.toml`.
- Context-aware: More discreet in public/shared contexts.

### 6. Failure Culture
- "I don't know" is always accompanied by what was checked.
- Mistakes are corrected explicitly: "I was wrong. Here's why. Here's the correct answer."
- Never vague. Never defensive. Never over-apologize.
- User corrections are always authoritative.

### 7. Proactive Agent
- Starts silent (Week 1: observation only).
- Week 2-4: Gentle offers, low-stakes, explicit asks.
- Month 2-3: Pattern-based permission offers.
- Month 6+: Autonomous assistance within granted permissions.
- **Attention budget:** Urgent > Important > Helpful > Optional. Never floods.
- Notification fatigue detection: If 3+ dismissals in a row, pause proactivity.

### 8. Knowledge Graph
- SQLite-based, single file, local-first.
- Entities, Facts (directed temporal edges), Sources (provenance), Preferences.
- Temporal facts: `valid_from`, `valid_until`. History is preserved.
- Confidence scores: 0.0-1.0. Facts color-coded by confidence.
- Obsidian-compatible export/import (Markdown + YAML frontmatter + wiki-links).
- Nightly optimization: deduplication, contradiction resolution, dormant cleanup.

---

## Phase 1: Core Agent (Current Plan)

**Goal:** Build the foundational layer. The agent can start, hold a conversation, stream responses from an OpenAI-compatible endpoint, and manage memory.md.

**12 Tasks:**

| # | Task | Files | Key Output |
|---|------|-------|------------|
| 1 | Workspace scaffolding | `Cargo.toml`, 3 crate directories | Compiling workspace |
| 2 | Config system | `config.rs`, `default.toml` | TOML + env overrides |
| 3 | LLM client | `llm/client.rs`, `llm/types.rs` | Streaming + non-streaming HTTP |
| 4 | memory.md | `memory/loader.rs`, `memory/manager.rs` | Load, save, add, replace, remove |
| 5 | Context manager | `context.rs` | Sliding window conversation history |
| 6 | Personality | `personality.rs` | Transparent default, presets |
| 7 | Tool registry | `tools/registry.rs` | Dynamic registration, OpenAI schema |
| 8 | Chat server | `mimir-server/src/`, `routes/chat.rs` | Axum + SSE streaming |
| 9 | CLI integration | `mimir-cli/src/main.rs` | `ask`, `chat`, `status`, `memory` |
| 10 | Auto-directories | `config.rs`, `loader.rs` | First-run config + memory.md creation |
| 11 | Integration tests | `tests/integration_tests.rs` | Config + memory coverage |
| 12 | Finalization | All `Cargo.toml` | Clean workspace build |

**Full plan with code in:** `docs/superpowers/plans/2025-05-20-mimir-phase-1-core-agent.md`

---

## Phase 2+: Roadmap Summary

| Phase | Focus | Duration | Key Deliverables |
|-------|-------|----------|-----------------|
| 1 | Core Agent | 4-6 weeks | CLI, chat, LLM, memory.md |
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

### Key VISION Docs (if you need to reference)
- `VISION/00-Overview/Vision-Statement.md` — Core premise and principles
- `VISION/01-Core-Agent/Personality.md` — Personality system
- `VISION/01-Core-Agent/Memory-System.md` — memory.md design
- `VISION/02-Knowledge-Graph/Learning-Modes.md` — Explicit vs casual learning
- `VISION/02-Knowledge-Graph/Temporal-Facts.md` — Temporal storage model
- `VISION/04-Reasoning-Engine/Technical-Design.md` — Investigation threads, meta-threads
- `VISION/05-Proactive-Agent/User-Experience.md` — Trust ladder
- `VISION/08-Architecture/Permission-Model.md` — Permission levels

---

## How to Start Implementing

1. **Clone the repo:** `git clone https://github.com/BhavsarDevansh/Mimir.git`
2. **Read the Phase 1 plan:** `docs/superpowers/plans/2025-05-20-mimir-phase-1-core-agent.md`
3. **Start with Task 1** (workspace scaffolding) and work through each task in order
4. **Each task is 2-5 minutes of work** — write failing test, run it, implement, verify, commit
5. **TDD throughout** — every feature starts with a failing test

---

## Environment Prerequisites

- Rust toolchain (latest stable, edition 2024)
- SQLite development libraries
- An OpenAI-compatible API key (or local model endpoint)
- Git

---

## Success Criteria for Phase 1

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `mimir status` shows correct config and memory.md loaded
- [ ] `mimir ask "hello"` returns a coherent response from LLM
- [ ] `mimir chat` starts an interactive session
- [ ] `mimir memory` displays current memory.md contents
- [ ] Server starts on `http://127.0.0.1:8080`
- [ ] SSE streaming endpoint works for chat

---

## Notes for Future Sessions

- The VISION docs are the authoritative design reference. If a decision is unclear, check there first.
- The Phase 1 plan contains exact code for every step. Use it as a blueprint, not a suggestion.
- When in doubt: ask the user. The design prioritizes user agency and transparency.
- This is a complex project. Each phase produces working, testable software on its own.
- The project is named after Mimir. Use "Mimir" in user-facing strings, "mimir" in code/package names.

---

*End of Implementation Context*
