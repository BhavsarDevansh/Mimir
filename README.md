# Mimir

**A persistent personal intelligence that learns from your life, reasons across your data, and becomes more useful the longer you use it.**

Named after the Norse god Mimir — the keeper of wisdom whose severed head preserved all knowledge and gave counsel to the gods. Mimir remembers everything, connects disparate facts, and helps you navigate the labyrinth of your own life.

## What Is Mimir?

Mimir is not a chatbot. It is a stateful, ever-learning companion that:

- **Learns implicitly** — observes your patterns, extracts facts, and builds a persistent knowledge graph of your life
- **Reasons intelligently** — investigates complex questions across multiple data sources, showing its work in real time
- **Acts proactively** — earns your trust over time, then anticipates your needs before you ask
- **Stays private** — local-first architecture. Your data stays on your device. No cloud intermediary.

## Core Principles

1. **Persistence over ephemerality** — Every interaction, fact, and preference is stored, versioned, and retrievable
2. **Implicit learning** — The agent observes, generalizes, and adjusts without requiring explicit training
3. **User sovereignty** — You can inspect, edit, and delete anything it knows. The knowledge base is yours
4. **Thoroughness** — When answering, it investigates all available avenues rather than settling on the first plausible answer
5. **Proactivity** — As confidence in its model of you grows, it anticipates needs rather than only responding to prompts
6. **Openness** — OpenAI-compatible API endpoint for all LLM needs; pluggable connectors for services

## Architecture

Mimir is built in **Rust** with a modular, local-first architecture:

- **Core Agent** — CLI, chat interface, LLM orchestration, tool calling, skills, personality system, and working memory
- **HTTP Server** — Axum-based daemon with SSE streaming, session management, and graceful shutdown
- **Storage Layer** — SQLite for conversation history, skill metrics, configuration, and the knowledge graph
- **Knowledge Graph** — Live memory condensation, entity/fact storage, temporal reasoning, a category-first ontology (predicate aliases for verb canonicalization + Dewey categories with aliases and subtree retrieval for grouping), and event-driven regeneration via the unified background scheduler
- **Events & Reminders** — A lifecycle + recurrence overlay on facts that surfaces upcoming birthdays, appointments, deadlines, and tasks in the Upcoming memory section, with a deterministic scan job for auto-completion and recurring advancement (Issue #74)
- **LLM-orchestrated learning** — The LLM calls the `remember` tool during conversation to persist facts; the deterministic Rust pipeline (`normalize_and_insert`) enforces confidence, overwrite, and sensitive-fact policy. The Librarian Agent remains available as an on-demand extraction API (Issue #137)
- **Retrieval Agent** — Dedicated ephemeral research agents that investigate the knowledge graph and conversation history on behalf of the main agent before answering complex questions
- **Connectors (Phase 3, in progress)** — A pluggable service ingestion framework (`mimir-connectors`) that will sync email, calendar, and photo libraries into the knowledge graph as connector-provenanced facts. The crate, feature flags (`photos`/`calendar`/`gmail`), the DB-access boundary (via the `KnowledgeGraph` facade only, no direct `sqlx`), the `connectors` instance-registry table + facade methods (sync cursor, auth state, health), the `sources.connector_instance_id` provenance FK linking facts to their connector instance, the shared `normalize_and_insert` ingestion boundary (so connectors funnel through the same confidence/corroboration/sensitivity pipeline as chat), the full entity-resolution chain — exact name → alias → FTS5 fuzzy (score ≥ 0.9) → create new, type-filtered so noisy connector data never cross-merges (Issue #182), and the runtime `Connector` trait + data types (Issue #183) — the async, object-safe contract every connector implements with two-step DB-free ingestion (`sync` → `extract` → supervisor-owned `normalize_and_insert`) — and the `ConnectorRegistry` + multi-backend factory dispatch (Issue #184), which maps each `(connector_type, backend)` pair to a `ConnectorFactory` so new backends register with no schema change, many backends coexist under one type, and reliability stays per-type — and the `ConnectorSupervisor` supervised lifecycle (Issue #185), which owns one supervised background task per `Active` connector and centralises spawn-on-startup, restart-with-backoff, a circuit breaker, auth-expiry pausing, graceful shutdown, and cursor persistence (each cycle runs `sync` → `extract` → `normalize_and_insert` in an isolated sub-task so a connector panic is contained) — and manual sync triggering (Issue #186), whereby `ConnectorSupervisor::trigger_sync` preempts a connector's polling interval with caller-supplied options (`--full`/`--since`), serialises concurrent triggers per connector via a one-permit semaphore, and returns the cycle's outcome — are in place; the secret store, daemon wiring, and backend implementations land in later Phase 3 issues

## Installation

> Coming soon. For now clone and run with `cargo run`.

## Quick Start

```bash
# Start the daemon
mimir start

# Ask a one-shot question
mimir ask "What is the capital of France?"

# Chat interactively with conversation history
mimir chat

# Check daemon status and configuration
mimir status

# View the live condensed memory block
mimir memory

# Force memory condensation immediately
mimir memory --refresh

# Query the knowledge graph audit log
mimir kb audit --entity "Alice" --change-type status_change

# List sensitive facts awaiting confirmation
mimir kb pending

# Confirm or reject a pending sensitive fact
mimir kb confirm 42
mimir kb reject 42 --reason "entered in error"

# Stop the daemon gracefully
mimir stop
```

## Configuration

Mimir auto-initialises its config directory on first run. The main config file lives at:

```
~/.config/mimir/config.toml
```

You can override settings with environment variables (e.g. `MIMIR_BASE_URL`).

Run `mimir init` for a guided first-run setup including identity configuration and optional systemd user service installation.

> **Note:** The legacy `memory.md` file-backed memory system was removed in v0.37.0. Memory is now served live from the knowledge graph.

## Documentation

The full project vision, architecture, and design documentation lives in the `VISION/` directory:

- `00-Overview/` — Vision statement, user values, success criteria
- `01-Core-Agent/` — CLI/chat UX, personality system, skills framework
- `02-Knowledge-Graph/` — Data model, temporal facts, learning modes, audit
- `03-Connectors/` — Connector framework, supported services, auth patterns
- `04-Reasoning-Engine/` — Investigation model, meta-threads, real-time streaming
- `05-Proactive-Agent/` — Trust ladder, pattern recognition, attention management
- `06-Vision-Tracking/` — Object detection, spatial memory
- `07-Journeys/` — End-to-end user scenarios and examples
- `08-Architecture/` — Security, privacy, deployment, integration points
- `09-Roadmap/` — Phased implementation plans

## License

[GNU General Public License v3.0](LICENSE)

Mimir is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

## Contributing

Contributions are welcome! See `CONTRIBUTING.md` (coming soon) for guidelines.

## Acknowledgments


- Named after **Mimir**, the Norse keeper of wisdom whose severed head preserved all knowledge
- Built with Rust, SQLite, and an OpenAI-compatible LLM of your choice
