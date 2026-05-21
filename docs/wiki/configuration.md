# Configuration

Mimir stores its settings in a TOML file. You can edit the file directly or override individual values with environment variables.

## Config File Location

The default path is platform-dependent:

- **Linux:** `~/.config/mimir/config.toml`
- **macOS:** `~/Library/Application Support/mimir/config.toml`
- **Windows:** `%APPDATA%\mimir\config.toml`

You can also pass a custom path when starting Mimir (feature coming in a future release).

## Creating the Config File

If the file does not exist, Mimir starts with safe compiled-in defaults. To create one manually:

```bash
mkdir -p ~/.config/mimir
cat > ~/.config/mimir/config.toml << 'TOML'
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-4o"
max_tokens = 4096
temperature = 0.2

[agent]
name = "Mimir"
proactivity = "important_only"
verbose_reasoning = false

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
TOML
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
| `MIMIR_MEMORY_ENABLED` | Enable working memory | `true` or `false` |
| `MIMIR_MEMORY_CHAR_LIMIT` | Character budget for memory.md | `2500` |
| `MIMIR_MEMORY_AUTO_MANAGE` | Auto-truncate old memory | `true` or `false` |
| `MIMIR_MEMORY_TEMPORAL_HORIZON` | Days of memory to retain | `30` |

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
