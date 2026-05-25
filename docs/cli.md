# CLI Architecture

## Overview

The `mimir` binary provides a command-line interface for interacting with Mimir. It operates in two modes:

- **Daemon mode** (`mimir start`): runs the persistent HTTP server in the foreground
- **Client mode** (`mimir ask`, `mimir chat`, etc.): interacts with Mimir's subsystems

Currently, client-mode commands link `mimir-core` directly for LLM, memory, context, and personality operations. In a future release, they will communicate with the daemon via HTTP.

## Architecture

```text
mimir (single binary)
├── main.rs         — Dispatch: daemon or client based on subcommand
├── cli.rs          — Command definitions (clap)
├── commands.rs     — Tool & Skill subcommand handlers
├── start.rs        — Daemon launcher (in-process Axum server)
├── ask.rs          — Single-shot query
├── chat.rs         — Interactive REPL
├── status.rs       — System status
├── memory_cmd.rs   — Memory viewer
├── init.rs         — First-run bootstrap
└── skills_permissions_config.rs — Skill permission persistence
```

### Library Crates (code organisation, not separate binaries)

| Crate | Type | Role |
|-------|------|------|
| `mimir-core` | library | LLM client, config, memory, context, personality, tools, skills |
| `mimir-server` | library | Axum routes, state, middleware |
| `mimir` | binary | Single entry point — dispatches daemon or client mode |

## Subcommands

### `mimir start`

Runs the Mimir HTTP server in the foreground (in-process, no separate binary). Use systemd or a process manager for backgrounding. Reads `bind_addr` from `[server]` config.

### `mimir init`

Creates Mimir directories and default configuration files. Idempotent.

On **Linux**, after creating config and memory files, you are prompted:
`Install systemd user service for auto-start? [y/N]:`.
Answering **yes** generates a hardened systemd user service file,
runs `systemctl --user daemon-reload`, and `systemctl --user enable --now mimir`.
If any step fails, manual `systemctl` instructions are printed as a fallback.

On **macOS**, a note about future launchd support is shown.
On **Windows**, the step is skipped silently.

### `mimir ask <query>`

Sends a single query to the configured LLM. Supports:
- `--no-stream` / `-n`: Non-streaming response
- `--model <model>`: Model override
- `--verbose` / `-v`: Token usage output
- `--incognito`: Skip context persistence
- `--personality <name>`: Personality preset override
- Piped stdin: Prepended as context

### `mimir chat`

Interactive REPL with:
- Persistent history at `~/.config/mimir/history.txt`
- Multi-line input (trailing `\` to continue)
- Built-in commands: `/exit`, `/clear`, `/memory`, `/status`, `/help`
- Ctrl+C during input exits, Ctrl+C during streaming aborts
- Conversation context managed via `ContextManager`

### `mimir status`

Displays:
- Config path and existence
- LLM endpoint and model
- LLM connectivity (via `/models` endpoint check)
- Memory.md path, character count, and usage percentage

### `mimir memory`

Loads and prints `memory.md` content to stdout.

## Key Design Decisions

- **Single binary**: The `mimir` binary contains both the daemon and client code. `mimir start` runs the Axum server in-process; no separate `mimir-server` binary is needed.
- **Daemon mode**: The server reads `bind_addr` from `[server]` config (default: `127.0.0.1:8080`) and listens for HTTP connections. systemd manages backgrounding and restarts.
- **Direct library linkage (current)**: CLI commands talk to `mimir-core` directly, bypassing the HTTP server. This will be refactored to use `mimir-client` for daemon communication in a future release.
- **LlmClient pooling**: Each command creates its own `LlmClient` with a single-worker pool.
- **Incognito mode**: Skips `ContextManager` persistence; uses `LlmClient` for one-shot operations.
- **REPL session**: Uses a single `ContextManager` session for the REPL duration with in-memory conversation history.
