# Configuration System

> **Scope:** `mimir-core/src/config.rs`, `config/default.toml`
> **Last updated:** 2026-05-21

## Architecture

The configuration subsystem lives in `mimir-core` and is the single source of truth for all runtime settings. It is loaded once at startup (hot-reload is deferred to a future issue) and consumed by the CLI, server, and core agent modules.

Precedence (highest wins):

1. `MIMIR_*` environment variables
2. User TOML file (`~/.config/mimir/config.toml`)
3. Compiled-in defaults (`config/default.toml`)

## API Reference

### `Config::load(path: Option<&Path>) -> Result<Self, ConfigError>`

- **`path = Some(p)`** — reads `p` as TOML. File must exist or an error is returned.
- **`path = None`** — resolves the platform config directory via `dirs::config_dir()`, appends `mimir/config.toml`, and reads it if present. If the file is missing, compiled defaults are used silently.

After file loading, all `MIMIR_*` environment variables are applied.

### `Config::save(&self, path: &Path) -> Result<(), anyhow::Error>`

Serialises the current configuration to pretty-printed TOML and writes it to disk. Parent directories are created automatically if missing.

### `Config::config_path() -> Option<PathBuf>`

Convenience wrapper that returns `dirs::config_dir() / "mimir" / "config.toml"`.

## Types

| Type | Key Fields | Notes |
|------|-----------|-------|
| `Config` | `llm`, `agent`, `memory` | Top-level container |
| `LlmConfig` | `endpoint`, `api_key`, `model`, `max_tokens`, `temperature` | `temperature` is `f32` |
| `AgentConfig` | `name`, `proactivity`, `verbose_reasoning` | `proactivity` is an enum |
| `MemoryConfig` | `enabled`, `char_limit`, `auto_manage`, `temporal_horizon` | `temporal_horizon` is `u8` days |
| `Proactivity` | `Never`, `ImportantOnly`, `Always` | Serialises as `snake_case` |

## Environment Variable Mapping

| Variable | Target Field | Type |
|----------|-------------|------|
| `MIMIR_LLM_API_KEY` | `llm.api_key` | `String` |
| `MIMIR_LLM_ENDPOINT` | `llm.endpoint` | `String` |
| `MIMIR_LLM_MODEL` | `llm.model` | `String` |
| `MIMIR_LLM_MAX_TOKENS` | `llm.max_tokens` | `u16` |
| `MIMIR_LLM_TEMPERATURE` | `llm.temperature` | `f32` |
| `MIMIR_AGENT_NAME` | `agent.name` | `String` |
| `MIMIR_AGENT_PROACTIVITY` | `agent.proactivity` | `Proactivity` |
| `MIMIR_AGENT_VERBOSE_REASONING` | `agent.verbose_reasoning` | `bool` |
| `MIMIR_MEMORY_ENABLED` | `memory.enabled` | `bool` |
| `MIMIR_MEMORY_CHAR_LIMIT` | `memory.char_limit` | `u16` |
| `MIMIR_MEMORY_AUTO_MANAGE` | `memory.auto_manage` | `bool` |
| `MIMIR_MEMORY_TEMPORAL_HORIZON` | `memory.temporal_horizon` | `u8` |

Invalid numeric or boolean values are ignored silently. Invalid `MIMIR_AGENT_PROACTIVITY` values produce `ConfigError::InvalidProactivity` with an actionable message.

## Validation Rules

- `Proactivity::from_str` accepts exactly `"never"`, `"important_only"`, and `"always"` (case-insensitive). Any other value yields `ConfigError::InvalidProactivity`.
- No additional runtime validation is performed on numeric ranges (e.g., `temperature` outside `0.0..=1.0`). Range checks can be added when the consuming modules define their requirements.

## Error Type

`ConfigError` (via `thiserror`) exposes three variants:

- `Io(std::io::Error)` — file read/write failures
- `Parse(toml::de::Error)` — malformed TOML
- `InvalidProactivity(String)` — illegal proactivity string
- `MissingConfigDir` — platform config directory unavailable

## Extending the Configuration

1. Add the new field to the appropriate struct (`LlmConfig`, `AgentConfig`, or `MemoryConfig`).
2. Update the `Default` implementation with a sensible baseline.
3. If the field should be overridable via environment variables, add a line in `Config::apply_env_overrides` following the existing `if let Ok(val)` pattern.
4. Update `config/default.toml` with the new default value.
5. Add a unit test asserting the default and, if applicable, a round-trip test via TOML.
6. Update this document and `docs/wiki/configuration.md`.
