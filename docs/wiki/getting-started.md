# Getting Started

## Prerequisites

- Rust toolchain (edition 2024, minimum version 1.85)
- SQLite development libraries (usually installed by default on most systems)
- An OpenAI-compatible LLM endpoint (local or remote)

## Installation

Clone the repository and build the workspace:

```bash
git clone https://github.com/BhavsarDevansh/Mimir.git
cd Mimir
cargo build --workspace --release
```

The `mimir` binary will be available at `target/release/mimir`.

## First Run

Run the guided initialisation:

```bash
./target/release/mimir init
```

This will:
1. Create the XDG config directory (`~/.config/mimir/`)
2. Generate a default `config.toml`
3. Optionally install and enable a systemd user service

## Configuration

The main config file lives at:

```
~/.config/mimir/config.toml
```

Key sections:

- `[llm]` — endpoint URL, model name, API key, temperature
- `[server]` — bind address (default `127.0.0.1:8080`)
- `[personality]` — default preset (`transparent`, `concise`, `warm`, `formal`)
- `[memory]` — character budget and auto-management for the knowledge graph memory

Environment variables override config values:

- `MIMIR_BASE_URL` — override the daemon base URL for CLI commands

## Quick Start

```bash
# Start the daemon in the foreground
mimir start

# In another terminal, ask a question
mimir ask "What is the capital of France?"

# Start an interactive chat session
mimir chat

# Check daemon health and config
mimir status

# View working memory
mimir memory

# Stop the daemon gracefully
mimir stop
```

## Running with systemd

If you opted into systemd integration during `mimir init`, the daemon will start automatically on login:

```bash
# Check service status
systemctl --user status mimir

# Start manually
systemctl --user start mimir

# Stop
systemctl --user stop mimir
```

## Troubleshooting

- **Daemon not running** — Client commands will prompt you to auto-start the daemon. If stdin is not a TTY, set `MIMIR_BASE_URL` to a running instance.
- **Port already in use** — Change `bind_addr` in `config.toml` or stop the existing process.
- **Config not found** — Run `mimir init` or ensure `~/.config/mimir/config.toml` exists.
