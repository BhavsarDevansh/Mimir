# Mimir

**A persistent personal intelligence that learns from your life, reasons across your data, and becomes more useful the longer you use it.**

Named after the Norse god Mimir — the keeper of wisdom whose severed head preserved all knowledge and gave counsel to the gods. Mimir remembers everything, connects disparate facts, and helps you navigate the labyrinth of your own life.

## What Is Mimir?

Mimir is not a chatbot. It is a stateful, ever-learning companion that:

- **Connects to your services** — email, calendar, photos, GitHub, Spotify, Home Assistant, Signal, and more
- **Learns implicitly** — observes your patterns, extracts facts, and builds a persistent knowledge graph of your life
- **Reasons intelligently** — investigates complex questions across multiple data sources, showing its work in real time
- **Acts proactively** — earns your trust over time, then anticipates your needs before you ask
- **Stays private** — local-first architecture. Your data stays on your device. No cloud intermediary.

## Example Interactions

```bash
> "When was I last in Rome?"

🔍 Investigating 5 sources...
✅ Found: May 5, 2025 (photos, calendar, email, messages, tour confirmation)

You were last in Rome May 3–7, 2025. I found a photo of you at the 
Colosseum on May 5th, and your Roman History Tour confirmation email.
```

```bash
> "Do I have time for coffee with Alice on Saturday?"

Checking your calendar, Alice's shared availability, your location 
patterns, and your mum's birthday dinner at 6 PM...

Yes — you have a free block 2–5 PM. Alice is usually free Saturday 
afternoons. I'll suggest 2 PM. Want me to send her a message?
```

```bash
🔔 Proactive: Flight Preparation

Your flight to Tokyo is in 6 hours. Based on your history:
- This is a 12-hour long-haul flight
- Tokyo forecast: 18°C and rain
- You usually pack noise-canceling headphones
- Your warmer clothes are in storage box B3

Want a checklist?
```

## Core Principles

1. **Persistence over ephemerality** — Every interaction, fact, and preference is stored, versioned, and retrievable
2. **Implicit learning** — The agent observes, generalizes, and adjusts without requiring explicit training
3. **User sovereignty** — You can inspect, edit, and delete anything it knows. The knowledge base is yours
4. **Thoroughness** — When answering, it investigates all available avenues rather than settling on the first plausible answer
5. **Proactivity** — As confidence in its model of you grows, it anticipates needs rather than only responding to prompts
6. **Openness** — OpenAI-compatible API endpoint for all LLM needs; pluggable connectors for services

## Architecture

Mimir is built in **Rust** with a modular, local-first architecture:

- **Core Agent** — CLI, chat interface, LLM orchestration, tool calling
- **Knowledge Graph** — Persistent SQLite-based graph of entities, facts, temporal data, and confidence scores
- **Connectors** — Pluggable adapters for external services (email, calendar, photos, Home Assistant, etc.)
- **Reasoning Engine** — Multi-threaded investigation with real-time streaming, hypothesis generation, and meta-thread conflict resolution
- **Proactive Agent** — Event monitoring, pattern recognition, and earned-trust proactive suggestions
- **Vision & Object Tracking** — Object detection and spatial memory for physical items (optional)

## Installation

> Coming soon. For now clone and run with `cargo run`.

## Quick Start

```bash
# Start the daemon
mimir start

# Connect your first service
mimir connector add gmail

# Ask a question
mimir ask "When was I last in Rome?"

# Chat interactively
mimir chat

# Browse what Mimir knows about you
mimir kb profile
```

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

## Trust Ladder

Mimir does not ask for broad permissions upfront. It observes, learns, and offers specific permissions when it has evidence they would be useful. You grant autonomy at your own pace:

1. **Observation** — Mimir reads data but never acts
2. **Gentle Offers** — "I noticed a flight email not in your calendar. Want me to add it?"
3. **Pattern Permissions** — "I've asked 5 times and you always said yes. Want me to do this automatically?"
4. **Autonomous Assistance** — Mimir acts within granted permissions, asks for anything outside

## License

[GNU General Public License v3.0](LICENSE)

Mimir is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

## Contributing

Contributions are welcome! See `CONTRIBUTING.md` (coming soon) for guidelines.

## Acknowledgments

- Inspired by the work of [Nous Research](https://nousresearch.com) and [Hermes Agent](https://github.com/nousresearch/hermes-agent)
- Named after **Mimir**, the Norse keeper of wisdom
- Built with Rust, SQLite, and an OpenAI-compatible LLM of your choice
