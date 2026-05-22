# CLI Architecture

## Overview

The `mimir` binary provides a command-line interface for interacting with Mimir's subsystems directly, without going through the HTTP server. It links `mimir-core` as a library for all LLM, memory, context, and personality operations.

## Architecture

```
mimir-cli (binary)
├── cli.rs          — Command definitions (clap)
├── main.rs         — Dispatch
├── commands.rs     — Tool & Skill subcommand handlers
├── skills_permissions_config.rs — Skill permission persistence
├── start.rs        — Server launcher
├── ask.rs          — Single-shot query
├── chat.rs         — Interactive REPL
├── status.rs       — System status
└── memory_cmd.rs   — Memory viewer
```

## Subcommands

### `mimir start`
Locates the `mimir-server` binary adjacent to the current executable or on `PATH`, and spawns it as a detached background process. Stdout and stderr are discarded.

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

- **Direct library linkage**: CLI talks to `mimir-core` directly, bypassing the HTTP server. This avoids unnecessary network overhead for terminal use.
- **LlmClient pooling**: Each command creates its own `LlmClient` with a single-worker pool, enabling concurrent handling without coupling across commands.
- **Incognito mode**: Skips `ContextManager` persistence; uses `LlmClient` for one-shot operations.
- **REPL session**: Uses a single `ContextManager` session for the REPL duration with in-memory conversation history.
