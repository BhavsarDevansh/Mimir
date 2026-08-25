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

# List personality presets
mimir personality list
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
# With session overrides (issue #81):
mimir chat --model gpt-4o --verbose --incognito --personality concise
```

### Session Flags

| Flag | Description |
|------|-------------|
| `-m, --model <model>` | Override the configured LLM model for this session |
| `-v, --verbose` | Print token usage after each response |
| `--incognito` | Skip context persistence and memory learning |
| `-p, --personality <name>` | Override the personality preset |

### Built-in Commands

| Command | Description |
|---------|-------------|
| `/exit` | Exit the REPL |
| `/clear` | Reset the conversation (start a new session) |
| `/memory` | Show live condensed memory from the knowledge graph |
| `/status` | Quick health check |
| `/history` | Resume a previous conversation |
| `/model [m]` | Show or set the LLM model override |
| `/personality [p]` | Show or set the personality preset |
| `/incognito [on\|off]` | Toggle incognito (skip persistence) |
| `/verbose [on\|off]` | Toggle token usage reporting |
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

Print the live condensed memory block — a ranked summary of your most important facts rendered **from the Knowledge Graph**, not a text file (see [Memory](memory.md)):

```bash
mimir memory
```

Memory is regenerated on demand when facts change, and the `memory.condensation` background hook only condenses when the LLM pool is idle so it never slows your conversations. Force a fresh condensation immediately:

```bash
mimir memory --refresh
```

`--refresh` prints the condensation run id and status; it does not print the memory block itself. Run `mimir memory` again afterwards to see the updated summary.

## `mimir personality list` — Personality Presets

List every available personality preset — the four built-ins (`transparent`, `concise`, `warm`, `formal`) plus your custom `.personality.md` files — as a table of `NAME`, `SOURCE`, and `DESCRIPTION` columns, sorted by name. Custom presets without a description show `-`.

```bash
mimir personality list
```

The command runs locally and works without a daemon. Broken preset files (for example an unclosed `---` frontmatter block) are skipped with a warning that names the file, and an unknown configured preset prints a warning instead of failing. See [Personality](personality.md) for how to add descriptions to custom presets.

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

Query and manage the Mimir knowledge graph. All commands talk to the daemon over HTTP — no direct SQLite access from the CLI.

Destructive operations (`forget`, `restore`, `trash --empty`) are loopback-only on the daemon: they work from the local CLI but return `403 Forbidden` for remote clients, so a daemon reachable from other devices can be inspected but not mutated remotely.

Every command supports `--json` for structured, scriptable output.

### `mimir kb query`

Query facts for an entity. Results are colour-coded by confidence (green >0.9, yellow 0.7–0.9, red <0.7).

```bash
mimir kb query "Alice"
mimir kb query "Alice" --predicate visited --min-confidence 0.8 --json
```

### `mimir kb show`

Show full detail for a single fact: sources, dependencies, and audit log.

```bash
mimir kb show 42
mimir kb show 42 --json
```

### `mimir kb edit`

Edit mutable fields on a fact. No `$EDITOR` mode — this is structured data.

```bash
mimir kb edit 42 --confidence 0.95
mimir kb edit 42 --valid-until 2026-12-31 --status Active
```

### `mimir kb browse`

Browse the knowledge graph starting from an entity. Outputs an indented tree.

```bash
mimir kb browse --entity "Alice" --depth 2
mimir kb browse --entity "Alice" --depth 3 --limit 100 --json
```

### `mimir kb profile`

Generate a Rust-built biography from the top-20 highest-confidence facts, grouped by category.

```bash
mimir kb profile
mimir kb profile --entity "Alice" --json
```

### `mimir kb audit`

Query the fact audit log with filters:

```bash
mimir kb audit --entity Alice --predicate visited --from 2025-01-01 --change_type created
```

#### Date filter format

`--from`/`--to` (and the same flags on `mimir kb forget`) accept:

- **RFC3339 with offset** — `2025-01-01T10:30:00Z`, `2025-01-01T10:30:00+02:00` (preserved as UTC).
- **Offsetless datetime** — `2025-01-01T10:30:00` or `2025-01-01 10:30:00` (interpreted in your local timezone).
- **Date only** — `2025-01-01` (midnight in your local timezone).

If you omit an offset, the time is treated as local, so a filter like `--from 2025-01-01T09:00:00` means 09:00 on your machine, not 09:00 UTC.

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

### `mimir kb pending`

List sensitive facts awaiting user confirmation (e.g. allergies, health details extracted from conversation). Pending facts are stored with `pending_confirmation = TRUE` and a `Disputed` status until confirmed or rejected. Facts ignored for longer than the configured retention window (default 7 days) are automatically hard-deleted by the `knowledge.pending_cleanup` background job.

```bash
mimir kb pending
mimir kb pending --json
```

### `mimir kb confirm`

Confirm a pending sensitive fact. Flips its status to `Active` and sets confidence to `1.0`, then triggers the inference cascade as if it were a freshly-inserted explicit fact.

```bash
mimir kb confirm 42
mimir kb confirm 42 --json
```

### `mimir kb reject`

Reject a pending sensitive fact. The fact is hard-deleted and a `rejected` audit entry is recorded. An optional `--reason` is written into the audit log (`User rejected sensitive fact: <reason>`).

```bash
mimir kb reject 42
mimir kb reject 42 --reason "entered in error"
```

### `mimir kb heatmap`

Render a knowledge-density snapshot of the graph: totals (facts, entities, average confidence), top entities and predicates by fact count, facts per month, and the confidence distribution (explicit / connector / inference / casual bands). Trashed facts are excluded.

```bash
mimir kb heatmap
mimir kb heatmap --json
```

### `mimir kb reset`

Wipe the entire knowledge graph with an explicit confirmation flow: live entity/fact counts in the warning, exact phrase `DELETE EVERYTHING` (case-sensitive), a 5-second countdown, then a daemon-side backup and hard delete. Requires a terminal; scripts use `mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"` instead.

```bash
mimir kb reset
```

## `mimir connector` — Connector Commands

Manage connector instances (email, calendar, photos) through the daemon over HTTP. Every command supports `--json` for structured, scriptable output, and slug-based commands resolve slugs against the daemon's instance list.

### `mimir connector add`

Register a new connector instance. The instance is created in `Setup` — run `mimir connector resume` to activate it, then `sync` to ingest.

**Interactive wizard (recommended for first-time setup):** run `mimir connector add` with no arguments, and it guides you through everything — pick the connector type from the daemon's catalog (e.g. `Email (imap)`), confirm the display name (defaults to the type, e.g. `Email`), confirm the slug (defaults to the name, e.g. `email`), answer the per-type questions, and choose authentication. Email provider presets (issue #400) pre-fill the provider defaults — Gmail (`imap.gmail.com:993` / `INBOX`, Google OAuth endpoints and scope pre-filled, OAuth browser login recommended), Outlook / Office 365 (`outlook.office365.com:993`, asks which Microsoft account type you connect — personal → `/consumers/`, work or school → `/organizations/`, either → `/common/` — and pre-fills the matching Microsoft login endpoints + IMAP scope, OAuth 2.0 only — Microsoft retired app passwords for IMAP), Yahoo (`imap.mail.yahoo.com:993`, app password), Proton Mail Bridge (`127.0.0.1:1143`, app password), iCloud (`imap.mail.me.com:993`, app password), or Custom IMAP — and the calendar wizard offers Google Calendar, iCloud, Yahoo, and Custom CalDAV presets. Every email preset also asks the sync-mode and first-sync-backfill questions (issue #397). For OAuth presets the CLI launches your browser at the authorization URL (the URL is printed first, so you can also open it manually — but the browser must run on the machine running `mimir` because the PKCE callback redirects to `http://localhost:<port>/callback` on the same machine), the loopback redirect is handled automatically, and the exchanged tokens are stored by the daemon. App-password paths are offered where the provider supports them, and local backends (e.g. Photos) need no credential. Secrets (app passwords, OAuth client secrets) are prompted hidden exactly once — there is no confirmation re-entry, so a single masked input is all a secret prompt expects (issue #399). The wizard only runs on a terminal — with piped input, it fails fast and points you at the flag form below.

```bash
mimir connector add                       # interactive wizard
```

The wizard registers the connector as read-only: it only imports data from the service, and write-back actions run only when you explicitly invoke `mimir connector act <slug>`.

The flag form gives the same result non-interactively, which is what scripts and power users use:

```bash
# Photos: watch a local directory
mimir connector add photos --backend local watch_dir=/home/me/Pictures

# Email over an app password (prompts for the password, then ingests it). The
# legacy `gmail` type name still works as an alias for `email`.
mimir connector add email --backend imap host=imap.gmail.com auth.kind=app_password auth.username=me@gmail.com

# Non-interactive: pipe the credential (recommended — keeps it out of shell history)
cat secret.txt | mimir connector add email --backend imap host=imap.fastmail.com auth.kind=app_password auth.username=me@fastmail.com --password-stdin

# Non-interactive: pass the credential via an environment variable (avoids the command line; load it from a protected source)
MIMIR_CONNECTOR_PASSWORD="$(cat secret.txt)" mimir connector add email --backend imap host=imap.fastmail.com auth.kind=app_password auth.username=me@fastmail.com

# Complex configs: full JSON object, with key=value overrides on top
mimir connector add calendar --backend caldav --config-json '{"calendar_url":"https://dav.example.com/cal","auth":{"kind":"app_password","username":"me@example.com"}}' --slug work-cal
```

Config is given as `key=value` pairs with dotted nesting (`auth.kind=app_password`) plus an optional `--config-json` base object. Scalar values are parsed as booleans, numbers, or strings; values starting with `[` or `{` are parsed as JSON arrays/objects (e.g. `'auth.scopes=["a","b"]'`), falling back to a plain string when the JSON does not parse (issue #289). OAuth configs (`auth.kind=oauth`) run the interactive PKCE login (A4 / #205) instead of prompting: the CLI opens the provider's authorize URL in your browser (printed first, so it can also be opened manually — but the browser must run on the machine running `mimir`, because the callback redirects to `http://localhost:<port>/callback` on the same machine), receives the redirect on an ephemeral loopback listener, exchanges the code, and POSTs the token bundle to the daemon — the instance becomes `authenticated`. The flow waits up to 5 minutes for the callback; if it times out, the flow aborts and you re-run the command to start a new login. `--slug` defaults to the connector type and `--name` to its title-cased form.

Before prompting for credentials, `add` asks the daemon for its catalog and fails fast if the requested `(connector_type, backend)` pair is not registered — no more discovering a typo after an interactive credential flow (issue #271).

**Secret hygiene (issue #270):** `--password <secret>` / `--token <secret>` are visible to any local user via the process list (`ps aux`) while the command runs and persist in shell history, terminal scrollback, and process supervisors' logs. Prefer channels that avoid the command line for real credentials: `--password-stdin` / `--token-stdin` (the whole piped stream is the secret, trailing newlines stripped — `cat secret.txt | mimir connector add ... --password-stdin`) or the `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN` environment variables (read by the CLI only, never by the daemon; the value stays in the process environment, so load it from a protected source such as `$(cat secret.txt)` rather than typing it into the command). Precedence per kind: flag, then stdin flag, then env var, then the interactive prompt. The flags are kept for script convenience, but treat them like API keys on a command line.

### `mimir connector catalog`

List the connector types and backends the daemon can actually construct. The daemon's registry is populated at startup from its build features, so this is the authoritative answer to "what can I connect?" — run it before `add`, and use it as the source for backend names:

```bash
mimir connector catalog          # table of supported type/backend pairs
mimir connector catalog --json   # structured output for scripts
```

### `mimir connector list` / `status`

```bash
mimir connector list              # table of every instance
mimir connector list --json
mimir connector status            # overview table
mimir connector status email     # detailed view of one instance
mimir connector status email --json
```

### `mimir connector sync`

Trigger a manual sync. `--since` accepts human durations (`30s`, `5m`, `12h`, `7d`) or bare seconds and conflicts with `--full`:

```bash
mimir connector sync email --since 7d
mimir connector sync photos --full --json
```

A connector that is not running (e.g. freshly added, still `Setup`) reports a 409 with a hint to run `mimir connector resume <slug>` first.

A connector whose sync mode has resolved to push (IMAP IDLE or a file watcher) reports a `CONNECTOR_PUSH_UNSUPPORTED` 409 — it syncs automatically, so manual sync is deferred. An `auto`-mode email connector whose mode has not resolved yet (its row shows `-` in `mimir connector list`) keeps manual sync as the force-retry until a cycle proves the mode (issue #475).

### `mimir connector auth`

Ingest credentials for an existing connector — completes an instance that was registered without credentials (a non-interactive `add`, or a credential the daemon later rejected) and re-auths after expiry, without `remove` + re-`add`:

```bash
cat secret.txt | mimir connector auth email --password-stdin   # app-password backend (piped secret)
MIMIR_CONNECTOR_TOKEN='api-token' mimir connector auth email   # API-token backend (env secret)
mimir connector auth email --password 'app-pw'                # app-password backend (flag — visible in `ps`/history)
mimir connector auth email                                    # interactive: pick the kind, then enter the secret
```

The credential kind comes from the flags (`--password` / `--token` / `--password-stdin` / `--token-stdin` are mutually exclusive), the `MIMIR_CONNECTOR_PASSWORD` / `MIMIR_CONNECTOR_TOKEN` environment variables (exactly one set), an interactive selection when none is given, or the `auth.kind` of a re-supplied config (`--config-json` / `key=value` pairs). The same secret-hygiene guidance as `add` applies: prefer `--*-stdin` or the env vars over `--password` / `--token` for real credentials. An `auth.kind=oauth` config runs the interactive PKCE login (A4 / #205) instead of prompting — the daemon does not expose the stored config on the wire, so the OAuth fields (`auth.auth_uri`, `auth.token_endpoint`, `auth.client_id`, optional `auth.client_secret` / `auth.scopes`) are re-supplied. `auth.scopes` is a JSON array — pass it as a JSON value in the `key=value` pair (issue #289):

```bash
mimir connector auth email auth.kind=oauth auth.auth_uri=https://accounts.google.com/o/oauth2/v2/auth auth.token_endpoint=https://oauth2.googleapis.com/token auth.client_id=... auth.username=you@gmail.com 'auth.scopes=["https://mail.google.com/"]'
```

After re-authing an expired connector, run `mimir connector resume <slug>` to restart its runner.

### `mimir connector pause` / `resume`

```bash
mimir connector pause email       # stop the runner
mimir connector resume email      # re-spawn the runner (activate)
```

### `mimir connector remove` vs `forget`

Both delete the instance and its stored credentials; they differ in what happens to the ingested facts:

- `remove` detaches provenance — the facts survive with degraded provenance.
- `forget` cascade-trashes every fact the connector sourced (recoverable from trash for 30 days) and then deletes the instance.

Both confirm interactively; pass `--yes` to skip:

```bash
mimir connector remove email --yes
mimir connector forget email --yes --json
```

### `mimir connector act`

Dispatch a write-back action (the Calendar connector supports `create_event`, `update_event`, `delete_event`):

```bash
mimir connector act calendar create_event '{"summary":"Lunch","start":"2026-08-12T12:00:00Z"}'
mimir connector act calendar delete_event --json-file payload.json --json
```

The output echoes the daemon's `ActionResult`: `native_id` (the remote resource href) and `message` (e.g. the new ETag).
