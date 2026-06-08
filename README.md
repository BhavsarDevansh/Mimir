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
- **Knowledge Graph** — Live memory condensation, entity/fact storage, temporal reasoning, and event-driven regeneration

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
