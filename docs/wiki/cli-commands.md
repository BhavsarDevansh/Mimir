# CLI Commands

Mimir provides a command-line interface for direct interaction with the LLM and system management. The `mimir` binary operates in two modes: daemon mode (`mimir start`) and client mode (all other commands).

## Quick Start

```bash
# First-time setup (creates directories and default config)
mimir init

# Start the daemon (runs in foreground; use systemd for backgrounding)
mimir start

# Ask a question
mimir ask "What is the capital of France?"

# Interactive chat
mimir chat

# Check system status
mimir status

# View memory
mimir memory
```

## `mimir start` — Start Daemon

Runs the Mimir HTTP server in the foreground. The server binds to the address configured in `[server].bind_addr` (default: `127.0.0.1:8080`).

```bash
mimir start
# Output: Mimir daemon listening on 127.0.0.1:8080
```

For production use, run as a systemd user service. See [Deployment Model](../../VISION/08-Architecture/Deployment-Model.md) for details.

## `mimir stop` — Stop Daemon

Triggers a graceful shutdown of the running Mimir daemon.

```bash
mimir stop
# Output: Mimir daemon stopped.
```

If the daemon is not running, the command will prompt to start it (see [Daemon Auto-Start](daemon-auto-start.md)).

## `mimir ask` — Single-Shot Queries

Send a one-off query to the LLM. Tokens stream to your terminal as they arrive.

### Options

| Flag | Description |
|------|-------------|
| `-n, --no-stream` | Wait for the full response instead of streaming |
| `-m, --model <model>` | Use a different model for this query |
| `-v, --verbose` | Show token usage after the response |
| `--incognito` | Don't save this interaction to context or memory |
| `-p, --personality <name>` | Override the personality preset |

### Piping

Pipe content into mimir to include it as context:

```bash
cat error.log | mimir ask "What went wrong?"
```

### Examples

```bash
# Quick query
mimir ask "Explain quantum computing in one paragraph"

# Non-streaming with usage stats
mimir ask -n -v "Summarise the README"

# Use a different model and personality
mimir ask -m gpt-4o-mini -p concise "List the top 5 Rust crates for CLI apps"

# Private query (no context saved)
mimir ask --incognito "What's the weather in Paris?"
```

## `mimir chat` — Interactive REPL

Start a conversation session with the LLM. Each message builds on the previous ones.

```bash
mimir chat
```

### Built-in Commands

| Command | Description |
|---------|-------------|
| `/exit` | Exit the REPL |
| `/clear` | Reset the conversation (start a new session) |
| `/memory` | Show live condensed memory from the knowledge graph |
| `/status` | Quick health check |
| `/history` | Resume a previous conversation |
| `/help` | Show available commands |

### Multi-line Input

End a line with `\` to continue on the next line:

```text
Mimir> What are the key differences between \
... Rust and Go for systems programming?
```

### Resuming Previous Conversations

Type `/history` to see a list of recent sessions. Use arrow keys or type to fuzzy-filter, then press Enter to resume a session. All messages from the last compaction point are replayed in the terminal.

### Keyboard Shortcuts

- **Ctrl+C** during input: Exit the REPL
- **Ctrl+C** during streaming: Abort the current response, return to prompt
- **Ctrl+D**: Exit the REPL

### History

Chat history is saved to `~/.config/mimir/history.txt` and loaded automatically between sessions.

## `mimir init` — First-Run Setup

Create the Mimir directory structure and default configuration files. This happens automatically on first use, but you can also run it explicitly:

```bash
mimir init
```

Output (Linux/XDG example):

```text
Created config directory: ~/.config/mimir
Created data directory:    ~/.local/share/mimir
Created default config:    ~/.config/mimir/config.toml

Next: set your API key in the config file or via MIMIR_LLM_API_KEY.
Then run: mimir ask hello
```

If everything already exists, it prints `Mimir is already initialized.` Existing files are never overwritten.

## `mimir status` — System Health

Check configuration and connectivity:

```bash
mimir status
```

Output includes:
- Config file location and existence
- LLM endpoint and model
- LLM connectivity (reachable/unreachable)
- Memory usage (characters used vs limit)

## `mimir memory` — View Memory

Print the contents of your persistent memory file:

```bash
mimir memory
```

This shows what Mimir remembers about you across sessions.

## `mimir tool` — Tool Management

Manage registered tools:

```bash
mimir tool list                    # List all tools
mimir tool enable <name>           # Enable a tool (set permission to Auto)
mimir tool disable <name>           # Disable a tool
mimir tool permission <name> <level>  # Set a tool's permission level
```

## `mimir skill` — Skill Management

Manage registered skills:

```bash
mimir skill list                   # List all skills
mimir skill list --origin builtin # Filter by origin
mimir skill show <name>            # Show full skill details
mimir skill add <path>             # Add a user skill from a Markdown file
mimir skill delete <name>          # Delete a user skill
mimir skill enable <name>          # Enable a skill
mimir skill disable <name>         # Disable a skill
```

## `mimir kb` — Knowledge Graph Commands

Query and manage the Mimir knowledge graph.

### `mimir kb audit`

Query the fact audit log with filters:

```bash
mimir kb audit --entity Alice --predicate visited --from 2025-01-01 --change_type created
```

### `mimir kb forget`

Forget facts at various granularities. Facts are soft-deleted to a trash bin with a 30-day expiry.

| Flag | Description |
|------|-------------|
| `--fact-id <id>` | Forget a single fact by ID |
| `--predicate <name>` | Forget all facts with this predicate |
| `--subject <name>` | Forget all facts where entity is the subject |
| `--entity <name>` | Forget all facts where entity is subject or object |
| `--source <name>` | Forget all facts from a given source |
| `--from <datetime>` | Forget facts created after this date |
| `--to <datetime>` | Forget facts created before this date |
| `--all` | Forget everything (full reset) |
| `--yes` | Skip confirmation for bulk (>100 facts) |
| `--confirm-sensitive` | Confirm deletion of sensitive predicates |
| `--archive` | On full reset, archive to trash instead of hard-delete |
| `--confirmation-phrase <phrase>` | Required "DELETE EVERYTHING" for full reset |

**Safeguards:**
- Bulk deletions of >100 facts require `--yes`.
- Deletions involving sensitive predicates (e.g. `allergy`, `password`) require `--confirm-sensitive`.
- Full reset requires typing `DELETE EVERYTHING` and creates a timestamped database backup.

```bash
# Forget a single fact
mimir kb forget --fact-id 42

# Forget all "visited" facts
mimir kb forget --predicate visited --yes

# Forget everything (creates backup)
mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"
```

### `mimir kb restore`

Restore facts from the trash bin.

```bash
# Restore a single fact by trash ID
mimir kb restore --trash-id 7

# Restore everything
mimir kb restore --all
```

### `mimir kb trash`

List or empty the trash bin.

```bash
# List trash contents
mimir kb trash

# Empty trash immediately
mimir kb trash --empty
```
