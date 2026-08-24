# Tool Registry

## Overview

The tool registry (`mimir-core::tools`) provides dynamic discovery, registration, and invocation of both native Rust tools and user-defined CLI tools. It exports tools in OpenAI-compatible function-calling format and enforces a permission model (`Auto`, `Ask`, `Disabled`) before execution.

## Architecture

```text
┌─────────────────────────────────────────┐
│           ToolRegistry                  │
│  ┌─────────────┐  ┌─────────────────┐  │
│  │  Built-ins  │  │   CLI Tools     │  │
│  │  (Native)   │  │  (tools.toml)   │  │
│  └─────────────┘  └─────────────────┘  │
│              │                          │
│         RwLock<HashMap<...>>           │
│              │                          │
│     ┌────────┴────────┐                │
│     │  ToolMetadata   │                │
│     │  + permission   │                │
│     └────────┬────────┘                │
│              │                          │
│         execute(name, args, ctx)        │
│     incognito guard + permission check  │
│     → factory rebuild (if registered)   │
│     → delegate to Tool::execute         │
└─────────────────────────────────────────┘
```

## Core Types

### `Tool` Trait

Object-safe async trait (via `async-trait`) that every tool implements:

| Method | Purpose |
|--------|---------|
| `name()` | Unique identifier |
| `description()` | LLM-facing explanation |
| `parameters_schema()` | JSON Schema object for arguments |
| `permission()` | Default `ToolPermission` |
| `is_write_tool()` | Whether the tool mutates persistent state (default `false`). Reports write capability to the registry; `ToolRegistry::execute` uses it to block write-capable tools during incognito turns (issue #155). |
| `execute(args)` | Async invocation with `serde_json::Value` |

### `ToolRegistry`

- Thread-safe via `RwLock<HashMap<String, ToolEntry>>`
- Methods: `register`, `register_native_with_factory`, `register_with_factory`, `get`, `metadata`, `set_permission`, `list`, `export_openai_tools`, `execute`
- Export helpers (issue #155): `export_openai_tools_filtered(allow_write_tools)` and `export_openai_tools_for_llm_with_writes(allow_write_tools)` filter write-capable tools out of the exported LLM tool set when `allow_write_tools` is `false`, while `is_write_tool(name)` only reports whether a named tool is write-capable. The execution-time guard that blocks write tools during incognito turns is the separate responsibility of `ToolRegistry::execute`. No built-in tool is currently write-capable (the `remember` tool was removed in #386 and replaced by the hooks engine), but the guard remains as defence-in-depth for future write tools.
- `with_builtins()` creates a pre-populated registry with `GetCurrentTimeTool` and `EchoTool`
- `execute(name, args, ctx)` applies the uniform checks — the incognito write-tool guard, the permission level, and factory resolution — before invoking the tool. `ToolContext` carries the per-request runtime dependencies (the request-resolved LLM and the incognito write-tool policy); tools registered with a `ToolFactory` (e.g. `retrieve_context`, issue #441) are rebuilt from the context on every call so request-scoped overrides are honoured, while the stored prototype instance continues to provide the schema for export.

### `ToolContext`

Per-request runtime dependencies passed to `ToolRegistry::execute`:

| Field | Purpose |
|-------|---------|
| `llm` | The request-resolved LLM backend (model/temperature overrides) |
| `allow_write_tools` | Whether write-capable tools may execute; incognito turns pass `false` so the registry blocks write tools uniformly (issue #155) |

Constructed with `ToolContext::new(llm, allow_write_tools)`.

### `ToolFactory`

`Arc<dyn Fn(&ToolContext) -> Arc<dyn Tool> + Send + Sync>` — rebuilds a tool with per-request runtime dependencies. Registered via `register_native_with_factory` / `register_with_factory`; when present, `execute` calls the factory with the request context instead of using the stored instance.

### `ToolPermission`

| Level | Behaviour |
|-------|-----------|
| `Auto` | Execute immediately |
| `Ask` | Return `ToolError::PermissionDenied` (prompting deferred) |
| `Disabled` | Return `ToolError::Disabled` |

### `ToolOutput`

Structured result carrying `result`, `error`, `stdout`, `stderr`, and `exit_code`. `to_llm_text()` renders a compact plaintext representation to minimise token usage in the LLM context.

### `ToolError`

Centralised error enum covering permission, timeout, invalid arguments, missing tools, CLI process failures, and schema errors, plus `BlockedIncognito` for write-capable tools refused during incognito turns (issue #155).

## Built-in Tools

### `GetCurrentTimeTool`
- Name: `get_current_time`
- No parameters
- Returns current UTC time in RFC 3339
- Permission: `Auto`

### `EchoTool`
- Name: `echo`
- Parameter: `message` (string, required)
- Returns the message unchanged
- Permission: `Auto`


### `GetWeatherTool`
- Name: `get_weather`
- Parameters:
  - `location` (string, required)
  - `date` (string, optional): `"current"` for current conditions only, or a `YYYY-MM-DD` date for a specific forecast day. Omit to get current conditions plus all available forecast days.
- Fetches current weather and up to a 3-day forecast from wttr.in
- **All measurements are metric-only**
- Returns current conditions: temperature (°C), feels-like temperature (°C), description, humidity %, wind speed (km/h), wind direction, UV index, visibility (km), pressure (mb)
- Returns forecast days (when requested or by default): date, min/max/avg temperature (°C), description, chance of rain %, chance of snow %, UV index
- Permission: `Auto`
- Network timeout: 15 seconds
- Unknown locations are detected even when wttr.in returns HTTP 200 with a plain-text error body
- Requests for unavailable forecast dates return an error listing the available dates

### `SearchConversationHistoryTool`
- Name: `search_conversation_history`
- Parameters:
  - `query` (string, required): terms to search for; all terms must match in any order, or wrap the whole query in double quotes for an exact phrase
  - `limit` (integer, optional): max results, default 5, max 20
  - `session_id` (integer, optional): restrict search to a single conversation
- Searches past conversation history via SQLite FTS5 and returns BM25-ranked contextual snippets
- Snippet markers: `<<<` and `>>>` highlight the matched term in context
- Permission: `Auto`

## CLI Tool Wrapper

### `CliToolConfig`

TOML-deserializable configuration:

| Field | Required | Default |
|-------|----------|---------|
| `name` | Yes | — |
| `description` | Yes | — |
| `executable` | Yes | — (must be absolute path) |
| `args` | No | `[]` |
| `schema` | Yes | — (JSON Schema object) |
| `timeout_secs` | No | `30` |
| `permission` | No | `Ask` |

### Argument Templating

Placeholders `{{key}}` in the `args` array are replaced with JSON argument values:

```toml
args = ["cat", "{{path}}"]
```

String values are unquoted for CLI ergonomics; non-string values are JSON-serialised.

### Security

- Executable path **must** be absolute
- No shell interpolation (direct `tokio::process::Command` spawn)
- `kill_on_drop(true)` prevents orphaned processes
- `tokio::time::timeout` enforces the configured timeout

## Configuration File

`~/.config/mimir/tools.toml` uses array-of-tables for CLI definitions and a `[permissions]` table for overrides:

```toml
[[tool]]
name = "my_script"
description = "Run my custom script"
executable = "/usr/local/bin/my_script"
args = ["{{input}}"]
schema = { type = "object", properties = { input = { type = "string" } }, required = ["input"], additionalProperties = false }
timeout_secs = 30
permission = "ask"

[permissions]
echo = "disabled"
get_current_time = "auto"
```

### Loading Behaviour

1. Register all built-in tools with `Auto` permission
2. For each `[[tool]]`, register a `CliTool` with the config's permission
3. Apply `[permissions]` overrides to any matching registered tool

### Saving Behaviour

`save_tools_config` persists:
- All `[[tool]]` definitions (with current permissions updated)
- `[permissions]` entries for any tool whose permission is not `Auto` or whose source is `Cli`

## OpenAI Schema Export

`export_openai_tools()` returns the modern function-calling format:

```json
{
  "type": "function",
  "function": {
    "name": "...",
    "description": "...",
    "parameters": { ... },
    "strict": true
  }
}
```

## CLI Management

`mimir` exposes:

| Command | Action |
|---------|--------|
| `mimir tool list` | Show name, source, permission |
| `mimir tool enable <name>` | Set `Auto` |
| `mimir tool disable <name>` | Set `Disabled` |
| `mimir tool permission <name> <auto&#124;ask&#124;disabled>` | Explicit set |

Changes are persisted to `tools.toml` immediately.

## Dependencies

- `async-trait` — object-safe async trait methods
- `tokio` (with `process` feature) — CLI subprocess spawning and timeout
- `serde` / `serde_json` — JSON schema and argument (de)serialization
- `toml` — `tools.toml` parsing
- `dirs` — platform config directory resolution
- `thiserror` — ergonomic error definitions

## Chat Integration

When the LLM backend receives a request via the `/chat` endpoint, enabled tools are forwarded in the OpenAI `tools` field. If the model responds with `tool_calls` instead of text:

1. Each tool call is extracted from the assistant message.
2. `ToolRegistry::execute` is invoked with the parsed JSON arguments and a per-request `ToolContext` (request-resolved LLM + incognito write-tool policy). Factory-registered tools such as `retrieve_context` are rebuilt from the context before execution, so per-request model/temperature overrides are honoured (issue #441).
3. Results are rendered with `ToolOutput::to_llm_text()` and sent back as `role: tool` messages.
4. A follow-up LLM call produces the final assistant response, which is persisted to the session.

This loop is handled within both the `chat_handler` (blocking) and `chat_stream_handler` (SSE) routes; only the final assistant text is stored in the conversation history. In streaming mode, tool-call deltas are accumulated across SSE chunks, the tools are executed when the usage block arrives, and the final response is streamed to the client.

## Future Work

- Actual interactive prompting for `Ask`-permission tools
- Connector-based tools (e.g., Email, Calendar)
- Skill registry (higher-level compositions of tools)
- Hot-reload of CLI tool definitions
