# CLI Architecture

## Overview

The `mimir` binary provides a command-line interface for interacting with Mimir. It operates in two modes:

- **Daemon mode** (`mimir start`): runs the persistent HTTP server in the foreground
- **Client mode** (`mimir ask`, `mimir chat`, etc.): interacts with Mimir's subsystems

All client-mode commands now talk to the daemon over HTTP through `mimir-client` — `ask`, `chat`, `kb`, `connector`, `memory`, and `status` all route through the daemon's Axum server (the daemon-guard auto-starts it when it is not running). No client-mode command touches the knowledge graph, memory, or LLM directly.

## Architecture

```text
mimir (single binary)
 ├── main.rs         — Dispatch: daemon or client based on subcommand
 ├── cli.rs          — Command definitions (clap)
 ├── commands.rs     — Tool & Skill subcommand handlers
 ├── cli_util.rs     — Shared CLI helpers (exit, client, JSON output)
 ├── start.rs        — Daemon launcher (in-process Axum server)
 ├── ask.rs          — Single-shot query
 ├── chat.rs         — Interactive REPL
 ├── kb/             — Knowledge-graph subcommand handlers
 ├── connector/      — Connector subcommand handlers
 ├── status.rs       — System status
 ├── memory_cmd.rs   — Memory viewer
 ├── init.rs         — First-run bootstrap
 └── daemon_guard.rs — Shared helper to ensure the daemon is running
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
- Built-in commands: `/exit`, `/clear`, `/memory`, `/status`, `/history`, `/help`
- Session options (issue #81): `--model`, `--verbose`/`-v`, `--incognito`, `--personality`/`-p`
- Runtime slash-commands: `/model [m]`, `/personality [p]`, `/incognito [on|off]`, `/verbose [on|off]` to show or toggle per-session state
- Ctrl+C during input exits, Ctrl+C during streaming aborts
- Conversation context managed via `ContextManager`

### `mimir status`

Displays:
- Config path and existence
- LLM endpoint and model
- LLM connectivity (via `/models` endpoint check)
- Memory.md path, character count, and usage percentage

### `mimir memory`

Prints the live condensed memory block from the knowledge graph.

### `mimir connector`

Manages connector instances through the daemon's connector routes (Phase 3 A3 / issue #204):

- `mimir connector add <type> --backend <b> [key=value...] [--config-json <json>] [--slug <s>] [--name <n>] [--password <p> | --token <t> | --password-stdin | --token-stdin]` — register a new instance (created in `Setup`; the credential is acquired *before* registration, so a canceled prompt or aborted OAuth flow exits with nothing created). Non-OAuth `auth.kind` configs resolve the credential per kind with the precedence flag → stdin flag → `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN` env var → interactive `inquire` prompt (issue #270: the stdin channel keeps secrets out of the process list and shell history, and the env channel avoids the command line but remains visible in the process environment; the flags remain for script convenience but leak via `ps`/history); `auth.kind=oauth` configs run the interactive PKCE loopback flow (A4 / #205) — the CLI opens the provider's authorize URL in the browser (printed first for headless sessions), receives the redirect on an ephemeral loopback listener, exchanges the code, and POSTs the token bundle to the daemon. Dotted keys nest (`auth.kind=app_password auth.username=me@example.com`); scalar values parse as booleans/numbers/strings, and values starting with `[` or `{` parse as JSON arrays/objects (e.g. `'auth.scopes=["a","b"]'`), falling back to a plain string when the JSON does not parse (issue #289).
- `mimir connector catalog [--json]` — list every `(connector_type, backend)` pair the daemon supports (issue #271). `add` pre-flights the requested pair against this catalog before prompting for credentials, so a typo'd backend fails immediately with the supported set instead of after an interactive flow.
- `mimir connector list [--json]` — every registered instance as a table.
- `mimir connector status [<slug>] [--json]` — detailed view of one instance, or the overview table when the slug is omitted.
- `mimir connector sync <slug> [--full | --since <duration>] [--json]` — manual sync; `--since` accepts `30s`/`5m`/`12h`/`7d` or bare seconds, and conflicts with `--full`. A `Setup`/paused instance reports the `CONNECTOR_NOT_RUNNING` 409 with an activation hint.
- `mimir connector auth <slug> [key=value...] [--config-json <json>] [--password <p> | --token <t> | --password-stdin | --token-stdin] [--json]` — ingest credentials for an existing instance (completes an unauthenticated `add`, or re-auths after expiry) without `remove` + re-`add`; the kind comes from the flags, the `MIMIR_CONNECTOR_*` env vars (exactly one set), an interactive selection, or the `auth.kind` of a re-supplied config, and the secret resolves with the same flag → stdin → env → prompt precedence as `add` (issue #270). An `auth.kind=oauth` config runs the interactive PKCE loopback flow (A4 / #205) instead of prompting.
- `mimir connector pause <slug> [--json]` / `resume <slug> [--json]` — stop/re-spawn the runner.
- `mimir connector remove <slug> [--yes]` — delete the instance and credentials, detaching provenance (facts survive).
- `mimir connector forget <slug> [--yes] [--json]` — cascade-forget: trash the connector's facts (recoverable 30 days), delete credentials and row.
- `mimir connector act <slug> <kind> [payload-json | --json-file <path>] [--json]` — write-back dispatch (e.g. Calendar `create_event`/`update_event`/`delete_event`).

All commands resolve slugs client-side against `GET /connectors` (there is no by-slug route). Credential prompts and destructive confirmations require a terminal; scripts pass `--password-stdin`/`--token-stdin` (piped), `MIMIR_CONNECTOR_PASSWORD`/`MIMIR_CONNECTOR_TOKEN` (env), `--password`/`--token` (visible in `ps`/history — last resort), or `--yes`.

### `mimir kb` date filters

KB audit and forget commands accept `--from`/`--to` date filters via
`mimir/src/kb/mod.rs::parse_datetime`. Strings with an explicit timezone offset (RFC3339,
e.g. `2020-06-15T10:30:00Z` or `...+02:00`) are preserved as UTC. Offsetless
datetimes (`2020-06-15T10:30:00`, `2020-06-15 10:30:00`) and date-only inputs
(`2020-06-15`) are interpreted in the CLI/daemon **local timezone** (sharing
`DailySchedule::naive_to_utc_local`), so user-authored local times behave
intuitively rather than being silently shifted to UTC (issue #168).

## Key Design Decisions

- **Single binary**: The `mimir` binary contains both the daemon and client code. `mimir start` runs the Axum server in-process; no separate `mimir-server` binary is needed.
- **Daemon mode**: The server reads `bind_addr` from `[server]` config (default: `127.0.0.1:8080`) and listens for HTTP connections. systemd manages backgrounding and restarts.
- **HTTP client mode (current)**: every client-mode command talks to the daemon over HTTP via `mimir-client`; the daemon owns the knowledge graph, memory, connectors, and LLM pool. `mimir start` runs the Axum server in-process; `mimir-client` is the single transport for client commands (`kb` and `connector` share the `cli_util` helpers).
- **LlmClient pooling**: Each command creates its own `LlmClient` with a single-worker pool.
- **Incognito mode**: Skips `ContextManager` persistence; uses `LlmClient` for one-shot operations.
- **REPL session**: Uses a single `ContextManager` session for the REPL duration with in-memory conversation history.
