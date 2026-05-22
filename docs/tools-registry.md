# Tool Registry

## Overview

The tool registry (`mimir-core::tools`) provides dynamic discovery, registration, and invocation of both native Rust tools and user-defined CLI tools. It exports tools in OpenAI-compatible function-calling format and enforces a permission model (`Auto`, `Ask`, `Disabled`) before execution.

## Architecture

```
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
│         execute()                      │
│     permission check                   │
│     → delegate to Tool::execute        │
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
| `execute(args)` | Async invocation with `serde_json::Value` |

### `ToolRegistry`

- Thread-safe via `RwLock<HashMap<String, ToolEntry>>`
- Methods: `register`, `get`, `metadata`, `set_permission`, `list`, `export_openai_tools`, `execute`
- `with_builtins()` creates a pre-populated registry with `GetCurrentTimeTool` and `EchoTool`

### `ToolPermission`

| Level | Behaviour |
|-------|-----------|
| `Auto` | Execute immediately |
| `Ask` | Return `ToolError::PermissionDenied` (prompting deferred) |
| `Disabled` | Return `ToolError::Disabled` |

### `ToolOutput`

Structured result carrying `result`, `error`, `stdout`, `stderr`, and `exit_code`. `to_llm_text()` renders a compact plaintext representation to minimise token usage in the LLM context.

### `ToolError`

Centralised error enum covering permission, timeout, invalid arguments, missing tools, CLI process failures, and schema errors.

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

`mimir-cli` exposes:

| Command | Action |
|---------|--------|
| `mimir tool list` | Show name, source, permission |
| `mimir tool enable <name>` | Set `Auto` |
| `mimir tool disable <name>` | Set `Disabled` |
| `mimir tool permission <name> <auto|ask|disabled>` | Explicit set |

Changes are persisted to `tools.toml` immediately.

## Dependencies

- `async-trait` — object-safe async trait methods
- `tokio` (with `process` feature) — CLI subprocess spawning and timeout
- `serde` / `serde_json` — JSON schema and argument (de)serialization
- `toml` — `tools.toml` parsing
- `dirs` — platform config directory resolution
- `thiserror` — ergonomic error definitions

## Future Work

- Actual interactive prompting for `Ask`-permission tools
- Connector-based tools (e.g., Gmail, Calendar)
- Skill registry (higher-level compositions of tools)
- Hot-reload of CLI tool definitions
