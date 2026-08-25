# CLI Architecture

## Overview

The `mimir` binary provides a command-line interface for interacting with Mimir. It operates in two modes:

- **Daemon mode** (`mimir start`): runs the persistent HTTP server in the foreground
- **Client mode** (`mimir ask`, `mimir chat`, etc.): interacts with Mimir's subsystems

All client-mode commands talk to the daemon over HTTP through `mimir-client` — `ask`, `chat`, `kb`, `connector`, `memory`, and `status` all route through the daemon's Axum server (the daemon-guard auto-starts it when it is not running). The one exception is `mimir personality list`, which reads local preset files and needs no daemon. No other client-mode command touches the knowledge graph, memory, or LLM directly.

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
 ├── personality_cmd.rs — Personality preset discovery
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

On **Linux**, after creating config and memory files, you are prompted: `Install systemd user service for auto-start? [y/N]:`. Answering **yes** generates a hardened systemd user service file, runs `systemctl --user daemon-reload`, and `systemctl --user enable --now mimir`. If any step fails, manual `systemctl` instructions are printed as a fallback.

On **macOS**, a note about future launchd support is shown. On **Windows**, the step is skipped silently.

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

### `mimir personality list`

Lists every available personality preset (built-in + custom) as a table with `NAME`, `SOURCE`, and `DESCRIPTION` columns, sorted by name. Custom presets without a description show `-`, and optional `description` frontmatter in custom preset files is parsed per `docs/personality-system.md` (issue #387). The command runs locally — presets are plain files in the config directory — so it needs no daemon. Non-fatal diagnostics (malformed preset files, an unknown configured preset) are printed to stderr while the command still exits successfully.

### `mimir connector`

Manages connector instances through the daemon's connector routes (Phase 3 A3 / issue #204):

- `mimir connector add` — interactive wizard: no arguments walk through type/backend selection (from the daemon's live catalog), display name (defaults to the type), slug (defaults to the slugified name), per-backend config, and authentication. Email presets (issue #400) pre-fill the provider defaults: Gmail (`imap.gmail.com:993`/`INBOX`, Google OAuth endpoints + `https://mail.google.com/` scope pre-filled — the user supplies their own OAuth client ID, OAuth first with app-password fallback), Outlook / Office 365 (`outlook.office365.com:993`, asks which Microsoft account type you connect — personal accounts → `/consumers/`, work or school in any organisational directory → `/organizations/`, either → `/common/`, or this organisational directory only → prompts for the tenant ID or domain and pre-fills tenant-specific endpoints — and pre-fills the matching Microsoft login endpoints + `https://outlook.office.com/IMAP.AccessAsUser.All offline_access` scope; the app registration's Supported account types must match the picked audience and the loopback redirect URI `http://localhost/callback` must be registered; OAuth 2.0 only — Microsoft retired app passwords for IMAP), Yahoo (`imap.mail.yahoo.com:993`, app password), Proton Mail Bridge (`127.0.0.1:1143`, app password), iCloud (`imap.mail.me.com:993`, app password), or Custom IMAP (free-form). Calendar presets (issue #400) do the same: Google Calendar (primary-calendar CalDAV URL computed from the account email + Google OAuth), iCloud and Yahoo (server URL defaults, app password), or Custom CalDAV. Every email preset asks the sync-mode decision (continuous push — recommended — vs polling every 5/15/30/60 minutes or a custom interval) and whether the first sync imports the existing mailbox or starts from "now" (issue #397). OAuth runs the shared PKCE loopback flow (A4 / #205): the URL is printed first and the browser is opened, so the login can be completed manually in a browser on the same machine (the callback redirects to `http://localhost:<port>/callback` on the same machine); app passwords and OAuth client secrets are prompted hidden, each exactly once (no confirmation re-entry — issue #399); local backends need no credential. Once credentials are ingested the wizard **auto-activates** the connector (`resume`) and sync starts immediately — polling cycles, or a push backfill of the existing inbox before it listens for new mail — and the summary prints the active state plus the resolved mode (an `auto` mode appears once the runner's first capability probe persists it). The wizard requires a TTY and exits with a pointer to the flag form when stdin is not a terminal. The created instance is read-only — it only imports data, and write-back runs only via an explicit `mimir connector act <slug>`.
- `mimir connector add <type> --backend <b> [key=value...] [--config-json <json>] [--slug <s>] [--name <n>] [--password <p> | --token <t> | --password-stdin | --token-stdin]` — the flag form of the same flow (created in `Setup`; the credential is acquired *before* registration, so a canceled prompt or aborted OAuth flow exits with nothing created; the instance stays inactive until you run `mimir connector resume <slug>` — explicit lifecycle for scripts, issue #397). Supplying only a connector type (or only `--backend`) does not start the wizard — it exits with a hint to complete the pair. Only non-OAuth flows with supplied credentials are fully non-interactive: non-OAuth `auth.kind` configs resolve the credential per kind with the precedence flag → stdin flag → `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN` env var → interactive `inquire` prompt (issue #270: the stdin channel keeps secrets out of the process list and shell history, and the env channel avoids the command line but remains visible in the process environment; the flags remain for script convenience but leak via `ps`/history); `auth.kind=oauth` configs are not fully scriptable — they run the interactive PKCE loopback flow (A4 / #205) — the CLI opens the provider's authorize URL in the browser (printed first, so it can be opened manually in a browser on the same machine — the callback redirects to `http://localhost:<port>/callback` on the same machine), runs the loopback listener, exchanges the code, and POSTs the token bundle to the daemon. Dotted keys (`auth.kind=app_password auth.username=me@example.com`); scalar values parse as booleans/numbers/strings, and values starting with `[` or `{` parse as JSON arrays/objects (e.g. `'auth.scopes=["a","b"]'`), falling back to a plain string when the JSON does not parse (issue #289).
- `mimir connector catalog [--json]` — list every `(connector_type, backend)` pair the daemon supports (issue #271). `add` pre-flights the requested pair against this catalog before prompting for credentials, so a typo'd backend fails immediately with the supported set instead of after an interactive flow.
- `mimir connector list [--json]` — every registered instance as a table, including the resolved sync `mode` (`push` / `polling`, issue #397). An `auto`-mode email connector shows `-` until its first capability probe has persisted the IMAP `IDLE` capability — the mode is never guessed before the probe completes (issue #397 review).
- `mimir connector status [<slug>] [--json]` — detailed view of one instance (including the resolved `mode`), or the overview table when the slug is omitted.
- `mimir connector sync <slug> [--full | --since <duration>] [--json]` — manual sync; `--since` accepts `30s`/`5m`/`12h`/`7d` or bare seconds, and conflicts with `--full`. A `Setup`/paused instance reports the `CONNECTOR_NOT_RUNNING` 409 with an activation hint; an instance whose mode is *resolved* to push reports `CONNECTOR_PUSH_UNSUPPORTED` 409 explaining that it syncs automatically via IMAP IDLE (or a file watcher) — polling-mode connectors keep manual sync, and an `auto`-mode email connector whose capability probe has not completed yet (the list shows `-`) also accepts manual sync as the force-retry until its mode is proven (issue #475).
- `mimir connector auth <slug> [key=value...] [--config-json <json>] [--password <p> | --token <t> | --password-stdin | --token-stdin] [--json]` — ingest credentials for an existing instance (completes an unauthenticated `add`, or re-auths after expiry) without `remove` + re-`add`; the kind comes from the flags, the `MIMIR_CONNECTOR_*` env vars (exactly one set), an interactive selection, or the `auth.kind` of a re-supplied config, and the secret resolves with the same flag → stdin → env → prompt precedence as `add` (issue #270). An `auth.kind=oauth` config runs the interactive PKCE loopback flow (A4 / #205) instead of prompting.
- `mimir connector pause <slug> [--json]` / `resume <slug> [--json]` — stop/re-spawn the runner.
- `mimir connector remove <slug> [--yes]` — delete the instance and credentials, detaching provenance (facts survive).
- `mimir connector forget <slug> [--yes] [--json]` — cascade-forget: trash the connector's facts (recoverable 30 days), delete credentials and row.
- `mimir connector act <slug> <kind> [payload-json | --json-file <path>] [--json]` — write-back dispatch (e.g. Calendar `create_event`/`update_event`/`delete_event`).

All commands resolve slugs client-side against `GET /connectors` (there is no by-slug route). Credential prompts and destructive confirmations require a terminal; scripts pass `--password-stdin`/`--token-stdin` (piped), `MIMIR_CONNECTOR_PASSWORD`/`MIMIR_CONNECTOR_TOKEN` (env), `--password`/`--token` (visible in `ps`/history — last resort), or `--yes`.

### `mimir kb heatmap`

Renders a knowledge-density snapshot of the knowledge graph as terminal bar charts: totals (facts, entities, average confidence), top entities and predicates by fact count, facts per month, and the confidence distribution (explicit / connector / inference / casual bands). Trashed facts are excluded. `--json` prints the raw `HeatmapResponse` for scripting (issue #69). Backed by the daemon's read-only `GET /kb/heatmap` aggregate; see `docs/kb-heatmap-reset.md` for the query semantics.

```bash
mimir kb heatmap
mimir kb heatmap --json
```

### `mimir kb merges`

Review surface for the nightly entity semantic-dedup pass (issue #282): `mimir kb merges list [--json]` shows pending `entity_merge_queue` rows (primary/duplicate names and types, the LLM's `suggested_action` and `llm_confidence`, queued time) from the daemon's loopback-gated `GET /kb/merges`; `mimir kb merges apply <id>` runs the existing entity-merge logic (repoint facts, move aliases/overlays/locations, delete the merged entity) via `POST /kb/merges/{id}/apply` and prints the actual survivor/merged ids; `mimir kb merges keep <id>` marks the pair `KeptSeparate` via `POST /kb/merges/{id}/keep`. Entities are never merged automatically — the nightly pass only queues suggestions.

### `mimir kb reset`

Dedicated full-wipe flow (issue #69): prints live entity/fact counts, requires the exact phrase `DELETE EVERYTHING` (case-sensitive) interactively, runs a 5-second countdown, then dispatches the shared `kb forget --all` path — the daemon re-validates the phrase, creates a timestamped backup under `~/.local/share/mimir/backups/`, and hard-deletes the graph. Requires a terminal; the non-interactive equivalent is `mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"`. See `docs/kb-heatmap-reset.md`.

### `mimir kb export` / `mimir kb import`

Obsidian-compatible Markdown exchange (issue #62): `mimir kb export` renders the knowledge graph as one `.md` file per entity (YAML frontmatter, wiki-links, `Dates`/`Relationships`/`Preferences`/`Facts` sections) to `--dir`, else `knowledge.export_dir`, else `~/AgentKnowledge`, and prints a summary; `--stdout` prints the files with `<!-- mimir: {name} -->` separators; `--json` dumps the raw bundle. `mimir kb import <path>` sends a vault directory to the daemon (`POST /kb/import`, loopback-gated), which parses, plans, and applies — `--dry-run` reports exactly what would change and writes nothing; re-importing skips exact existing triples. Imported facts use `source_type=Import` (0.80 confidence unless the file carries a `confidence: N` attribute) and flow through the shared pipeline (canonicalisation, corroboration, sensitivity gate, events overlay). See `docs/obsidian-export-import.md`.

### `mimir kb` date filters

KB audit and forget commands accept `--from`/`--to` date filters via `mimir/src/kb/mod.rs::parse_datetime`. Strings with an explicit timezone offset (RFC3339, e.g. `2020-06-15T10:30:00Z` or `...+02:00`) are preserved as UTC. Offsetless datetimes (`2020-06-15T10:30:00`, `2020-06-15 10:30:00`) and date-only inputs (`2020-06-15`) are interpreted in the CLI/daemon **local timezone** (sharing `DailySchedule::naive_to_utc_local`), so user-authored local times behave intuitively rather than being silently shifted to UTC (issue #168).

## Key Design Decisions

- **Single binary**: The `mimir` binary contains both the daemon and client code. `mimir start` runs the Axum server in-process; no separate `mimir-server` binary is needed.
- **Daemon mode**: The server reads `bind_addr` from `[server]` config (default: `127.0.0.1:8080`) and listens for HTTP connections. systemd manages backgrounding and restarts.
- **HTTP client mode (current)**: every client-mode command talks to the daemon over HTTP via `mimir-client`; the daemon owns the knowledge graph, memory, connectors, and LLM pool. `mimir start` runs the Axum server in-process; `mimir-client` is the single transport for client commands (`kb` and `connector` share the `cli_util` helpers). Each command resolves its transport once per invocation with this precedence: `MIMIR_BASE_URL` wins (remote daemon), then the Unix domain socket (`~/.local/share/mimir/mimir.sock`, issue #25), then TCP fallback (`server.bind_addr` or the compiled default) — see `docs/uds-transport.md`.
- **LlmClient pooling**: Each command creates its own `LlmClient` with a single-worker pool.
- **Incognito mode**: Skips `ContextManager` persistence; uses `LlmClient` for one-shot operations.
- **REPL session**: Uses a single `ContextManager` session for the REPL duration with in-memory conversation history.
