# Configuration

Mimir stores its settings in a TOML file. You can edit the file directly or override individual values with environment variables.

## Config File Location

The default path is platform-dependent:

- **Linux:** `~/.config/mimir/config.toml`
- **macOS:** `~/Library/Application Support/mimir/config.toml`
- **Windows:** `%APPDATA%\mimir\config.toml`

You can also pass a custom path when starting Mimir (feature coming in a future release).

## Creating the Config File

If the file does not exist, Mimir automatically creates the directory structure and a default config on first run. You can also run the following command to initialise explicitly:

```bash
mkdir -p ~/.config/mimir
cat > ~/.config/mimir/config.toml << 'TOML'   # not needed — `mimir init` does this for you
[llm]
endpoint = "https://api.openai.com/v1"
# Set your API key here, or use the MIMIR_LLM_API_KEY environment variable.
api_key = ""
model = "gpt-4o"
temperature = 0.2
TOML
```

Or simply:

```bash
mimir init
```

This creates:
- `~/.config/mimir/` — config directory
- `~/.local/share/mimir/` — data directory
- `~/.config/mimir/config.toml` — default config with helpful comments
- `~/.local/share/mimir/knowledge.db` — knowledge graph database

Existing files are never overwritten. Running `mimir init` again prints "Mimir is already initialized."

If you prefer to create the file manually:

```bash
mkdir -p ~/.config/mimir
cat > ~/.config/mimir/config.toml << 'TOML'
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"
temperature = 0.2

[agent]
name = "Mimir"
proactivity = "important_only"
verbose_reasoning = false
remember_debounce_seconds = 10

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30

[server]
bind_addr = "127.0.0.1:8080"
# socket_path = "~/.local/share/mimir/mimir.sock"  # Optional override; defaults to <data_dir>/mimir.sock on Unix (disabled on Windows)
TOML
```

### Setting Your API Key

The default config has `api_key = ""`. You must set it before using Mimir:

```bash
# Option 1: environment variable (recommended for security)
export MIMIR_LLM_API_KEY="sk-..."

# Option 2: edit the config file directly
# Open ~/.config/mimir/config.toml and set api_key = "sk-..."
```

## Environment Variables

Every field can be overridden at runtime without touching the config file. This is especially useful for secrets and CI/CD pipelines.

```bash
export MIMIR_LLM_API_KEY="sk-..."
export MIMIR_AGENT_PROACTIVITY="always"
mimir ask "Hello"
```

### Full variable list

| Variable | Description | Example |
|----------|-------------|---------|
| `MIMIR_LLM_API_KEY` | API key for the LLM provider | `sk-abc123` |
| `MIMIR_LLM_ENDPOINT` | OpenAI-compatible base URL | `https://api.openai.com/v1` |
| `MIMIR_LLM_MODEL` | Model identifier | `gpt-4o` |
| `MIMIR_LLM_MAX_TOKENS` | Maximum tokens per response | `4096` |
| `MIMIR_LLM_TEMPERATURE` | Sampling temperature | `0.2` |
| `MIMIR_AGENT_NAME` | Display name | `Mimir` |
| `MIMIR_AGENT_PROACTIVITY` | When the agent acts on its own | `never`, `important_only`, `always` |
| `MIMIR_AGENT_VERBOSE_REASONING` | Show chain-of-thought | `true` or `false` |
| `MIMIR_AGENT_REMEMBER_DEBOUNCE_SECONDS` | Debounce window for the `remember.chat` background hook | `10` |
| `MIMIR_MEMORY_ENABLED` | Enable working memory | `true` or `false` |
| `MIMIR_MEMORY_CHAR_LIMIT` | Character budget for condensed memory | `2500` |
| `MIMIR_MEMORY_AUTO_MANAGE` | Auto-truncate old memory | `true` or `false` |
| `MIMIR_MEMORY_TEMPORAL_HORIZON` | How many days ahead the upcoming-events section spans in the memory block | `30` |
| `MIMIR_SERVER_BIND_ADDR` | TCP bind address for the daemon | `127.0.0.1:8080` |
| `MIMIR_SERVER_SOCKET_PATH` | Unix domain socket path for local CLI | `<data-dir>/mimir.sock` on Unix |
| `MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES` | Comma-separated daily scan times (HH:MM) for the events job | `07:30,19:45` |
| `MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS` | How many days ahead the events scan looks for upcoming facts | `30` |
| `MIMIR_CONTEXT_DB_PATH` | Override the conversation-history database path | `/tmp/mimir/context.db` |
| `MIMIR_CONTEXT_COMPACTION_ENABLED` | Enable background session compaction (LLM summarisation of old turns) | `true` or `false` |
| `MIMIR_CONTEXT_COMPACTION_MAX_TURNS` | Older complete turns beyond this many are summarised and removed (clamped to one below `MIMIR_CONTEXT_MAX_TURNS` if set equal or higher) | `15` |
| `MIMIR_KNOWLEDGE_DB_PATH` | Override the knowledge-graph database path | `/tmp/mimir/knowledge.db` |
| `MIMIR_JOBS_DB_PATH` | Override the job-queue database path | `/tmp/mimir/jobs.db` |
| `MIMIR_GEOCODER_ENABLED` | Enable or disable geocoding entirely | `true` or `false` |
| `MIMIR_GEOCODER_ENDPOINT` | Base URL of the Nominatim instance | `https://nominatim.example.com` |
| `MIMIR_GEOCODER_CONTACT_EMAIL` | Contact email appended to the `User-Agent` (empty clears it) | `you@example.com` |
| `MIMIR_SECRETS_BACKEND` | Connector credential store: `file` or `keychain` | `file` |

## Connector Secrets Storage

The `[secrets]` section controls where connector credentials live:

```toml
[secrets]
backend = "file"  # or "keychain"
```

- `backend = "file"` (the default) stores each connector's credentials in a per-slug JSON file under `~/.local/share/mimir/secrets/` with `0600`/`0700` permissions, refused if the permissions are ever loosened.
- `backend = "keychain"` stores credentials in your operating system's credential store — macOS Keychain, Linux/FreeBSD/OpenBSD Secret Service (gnome-keyring / KWallet), or Windows Credential Manager — but requires a build with the `secrets-keyring` cargo feature on a supported target (Linux, FreeBSD, OpenBSD, macOS, or Windows). The feature is off by default because headless Linux boxes often have no Secret Service daemon. On an unsupported target, or in a build without the feature, the daemon refuses to start with an explanatory error rather than silently storing secrets in plaintext.
- Changing `backend` requires a process restart: the secret store is constructed once at startup and is not hot-reloaded.

## Geocoding

The `[geocoder]` section controls how Mimir turns place names and addresses into coordinates (and back):

```toml
[geocoder]
enabled = true
endpoint = "https://nominatim.openstreetmap.org"
# contact_email = "you@example.com"  # Optional: appended to the User-Agent (Nominatim policy)
```

- `enabled = false` turns geocoding off entirely — location facts are still stored, just without the missing coordinates or place name filled in.
- `endpoint` points at a self-hosted Nominatim instance for heavy use (recommended by Nominatim's usage policy).
- `contact_email` is appended to the `User-Agent` sent to the instance; setting it is recommended when using the public one.
- Changing `enabled`, `endpoint`, or `contact_email` requires a process restart: the geocoder is constructed once at startup and is not hot-reloaded.

## Proactivity Levels

- **`never`** — Mimir only responds when you explicitly ask.
- **`important_only`** — Mimir surfaces high-importance observations (default).
- **`always`** — Mimir proactively suggests actions and reminders.

## Troubleshooting

### "Invalid proactivity value"

You supplied something other than `never`, `important_only`, or `always` (case-insensitive). Check the spelling and try again.

### Config file is ignored

Make sure the file is valid TOML. A stray comma or missing quote will cause the parser to fail with a clear line number.

### Changes not reflected

Mimir loads configuration once at startup. Restart the process after editing the file.

## Hot-Reload

Mimir can hot-reload non-sensitive configuration settings without restarting. This allows you to tune personality, memory limits, context budgets, and agent behaviour on the fly.

### Which Settings Reload

These settings are reloaded when the config file changes or a `SIGHUP` signal is received:

- Personality preset (`personality.preset`)
- Memory character limit (`memory.char_limit`), auto-manage (`memory.auto_manage`), temporal horizon (`memory.temporal_horizon`)
- Context max tokens (`context.max_tokens`), max turns (`context.max_turns`)
- Agent name (`agent.name`), proactivity (`agent.proactivity`), verbose reasoning (`agent.verbose_reasoning`)
- LLM max tokens (`llm.max_tokens`), temperature (`llm.temperature`)
- Context compaction (`context.compaction.enabled`, `context.compaction.max_turns`) — the synchronous compact-before-trim path reads the live values; the background `session.compaction` hook is registered at daemon startup, so its window and enablement change only after a restart

These settings are **not reloaded** (they require a restart):

- LLM endpoint (`llm.endpoint`), API key (`llm.api_key`), model (`llm.model`)
- Server bind address (`server.bind_addr`), socket path (`server.socket_path`)

If you change a sensitive setting and trigger a reload, Mimir logs a warning and keeps the old value. No restart is forced.

### How to Trigger a Reload

**Method 1: Edit the config file**

Edit `~/.config/mimir/config.toml` and save. Mimir watches the config directory and picks up the change within one second.

```bash
# Change the personality to "concise"
sed -i 's/preset = ".*"/preset = "concise"/' ~/.config/mimir/config.toml
```

**Method 2: Send SIGHUP (Unix only)**

```bash
kill -SIGHUP $(pidof mimir)
```

This triggers an immediate reload of the config file.

The SIGHUP handler is registered as soon as the daemon starts, so a SIGHUP sent during the startup window (for example, right after `mimir start` returns) is caught and reloads the config instead of terminating the daemon (issue #369).

### What Happens on Error

- **Parse error** (invalid TOML): The old config is kept. Mimir logs a warning with the parse error details.
- **Sensitive field changed**: The reload is aborted. Mimir logs a warning stating which field was rejected.
- **I/O error** (file missing, permissions): The old config is kept. Mimir logs a warning.

In all cases, the server continues running with the last known good configuration.
