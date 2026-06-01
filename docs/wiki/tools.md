# Tools

Mimir can use **tools** — deterministic functions that perform specific tasks like telling the time, running a script, or querying data. Tools are exposed to the LLM in OpenAI-compatible format, so the agent can decide when to invoke them.

## Built-in Tools

Mimir ships with a small set of native tools:

| Tool | What it does | Permission |
|------|--------------|------------|
| `get_current_time` | Returns the current date and time | Auto |
| `echo` | Echoes back whatever you send it | Auto |
| `memory` | Updates `memory.md` (add, replace, remove entries) | Auto |
| `get_weather` | Fetches current weather for a location via wttr.in | Auto |

"Auto" means the tool runs immediately when the agent decides to use it.
### Weather Tool (`get_weather`)

The weather tool queries [wttr.in](https://wttr.in) to retrieve current conditions for any location. You can ask for a city name, airport code, or GPS coordinates.

When the agent invokes `get_weather`, it receives structured data including:
- Temperature in Celsius and Fahrenheit
- "Feels like" temperature
- Weather description (e.g., "Partly cloudy")
- Humidity, wind speed and direction
- UV index, visibility, and atmospheric pressure

Example queries:
- "What is the weather in London?"
- "Do I need an umbrella in Tokyo?"
- "How hot is it in New Delhi right now?"

## Adding Your Own CLI Tools

You can wrap any command-line program as a Mimir tool by editing `~/.config/mimir/tools.toml`.

### Example

```toml
[[tool]]
name = "weather"
description = "Fetches the weather for a given city."
executable = "/usr/local/bin/weather-cli"
args = ["--city", "{{city}}"]
schema = { type = "object", properties = { city = { type = "string" } }, required = ["city"], additionalProperties = false }
timeout_secs = 10
permission = "ask"
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique tool name |
| `description` | Yes | What the tool does (the LLM sees this) |
| `executable` | Yes | Absolute path to the program |
| `args` | No | Arguments, with `{{placeholders}}` for values |
| `schema` | Yes | JSON Schema describing the arguments |
| `timeout_secs` | No | How long to wait before killing the process (default: 30) |
| `permission` | No | `auto`, `ask`, or `disabled` (default: `ask`) |

### Placeholders

Use `{{key}}` in `args` to pass values from the LLM's JSON arguments:

```toml
args = ["--file", "{{path}}", "--count", "{{n}}"]
```

String values are passed unquoted. Numbers and booleans are JSON-encoded.

### Security

- The executable path **must** be absolute. Relative paths are rejected.
- There is no shell interpolation — arguments are passed directly.
- If the process hangs, Mimir kills it after the timeout.

## Permissions

Every tool has a permission level:

| Level | Behaviour |
|-------|-----------|
| **Auto** | The agent runs the tool immediately |
| **Ask** | The agent rejects the call for now (future: will prompt you) |
| **Disabled** | The tool cannot be used at all |

CLI tools default to **Ask** so you stay in control. Built-in tools default to **Auto**.

You can change permissions via the CLI or by editing `tools.toml`.

## Managing Tools from the Command Line

```bash
# List all tools
mimir tool list

# Enable a tool (Auto)
mimir tool enable echo

# Disable a tool
mimir tool disable echo

# Set a specific permission
mimir tool permission echo ask
```

Changes are saved to `~/.config/mimir/tools.toml` automatically.

### Permission Overrides in TOML

Instead of using the CLI, you can write:

```toml
[permissions]
echo = "disabled"
get_current_time = "auto"
my_custom_tool = "ask"
```

## How It Works

When the agent thinks a tool would help answer your question, it receives the tool's name, description, and parameter schema. It then generates a JSON argument object. Mimir checks the tool's permission:

1. **Disabled** → reject immediately
2. **Ask** → reject with a permission-denied message (prompting will come later)
3. **Auto** → execute and return the result to the agent

The agent sees the tool's output as compact plaintext and uses it to form its final response.
