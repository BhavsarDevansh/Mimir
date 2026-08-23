# Configuration System

> **Scope:** `mimir-core/src/config/`, `mimir-core/src/paths.rs`, `config/default.toml`
>
> **Last updated:** 2026-08-22

## Architecture

The configuration subsystem lives in `mimir-core` and is the single source of truth for all runtime settings. It is loaded once at startup (hot-reload is deferred to a future issue) and consumed by the CLI, server, and core agent modules.

Precedence (highest wins):

1. `MIMIR_*` environment variables
2. User TOML file (`~/.config/mimir/config.toml`)
3. Auto-initialised default config (if no file exists)
4. Compiled-in defaults (`config/default.toml`)

## API Reference

### `Config::load(path: Option<&Path>) -> Result<Self, ConfigError>`

- **`path = Some(p)`** — reads `p` as TOML. File must exist or an error is returned.
- **`path = None`** — resolves the platform config directory via `paths::config_path()`, reads the file if it exists. If the file is missing, `Config::init()` is called to bootstrap directories and write the default `config.toml`, then compiled defaults are returned.

After file loading, all `MIMIR_*` environment variables are applied.

### `Config::init() -> Result<InitResult, ConfigError>`

Creates the Mimir directory structure and default configuration files. Idempotent — subsequent calls return `InitResult::AlreadyInitialized` without overwriting existing files.

Returns `InitResult::Created { config_dir, data_dir, config_file }` on first call, or `InitResult::AlreadyInitialized` if everything already exists.

### `paths` module

Centralised XDG-aware path resolution in `mimir-core/src/paths.rs`:

| Function | Returns | Description |
|----------|---------|-------------|
| `config_dir()` | `Result<PathBuf, PathsError>` | `~/.config/mimir` |
| `data_dir()` | `Result<PathBuf, PathsError>` | `~/.local/share/mimir` |
| `cache_dir()` | `Result<PathBuf, PathsError>` | `~/.cache/mimir` |
| `config_path()` | `Result<PathBuf, PathsError>` | `config_dir()/config.toml` |
| `default_db_path()` | `Result<PathBuf, PathsError>` | `data_dir()/context.db` |
| `ensure_dir()` | `Result<(), PathsError>` | Idempotent `create_dir_all` |

All functions return `Result` with descriptive `PathsError` variants that explain how to troubleshoot (set `$HOME`, `$XDG_CONFIG_HOME`, etc.).

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
| `GeocoderConfig` | `enabled`, `endpoint`, `contact_email` | Controls the shared Nominatim geocoder (issue #227) |
| `SecretsConfig` | `backend` | Connector credential store: `file` (default) or `keychain` (issue #188) |
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

`ConfigError` (via `thiserror`) exposes four variants:

- `Io(std::io::Error)` -- file read/write failures
- `Parse(toml::de::Error)` -- malformed TOML
- `Paths(PathsError)` -- platform path resolution failure
- `InvalidProactivity(String)` -- illegal proactivity string

`PathsError` (via `thiserror`) exposes four variants:

- `MissingConfigDir` -- platform config directory unavailable
- `MissingDataDir` -- platform data directory unavailable
- `MissingCacheDir` -- platform cache directory unavailable
- `Io { path, source }` -- directory creation failure with path context

All `PathsError` variants include troubleshooting guidance in their error messages (e.g., "Ensure $HOME is set, or set $XDG_CONFIG_HOME to a valid path.").


## SecretsConfig

Controls which [`SecretStore`](connector-secret-store.md) backend the daemon uses for connector credentials (issue #188 / F11).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `backend` | `SecretsBackend` | `"file"` | `"file"` stores per-slug JSON files with `0600`/`0700` permissions (V1 default); `"keychain"` stores bundles in the OS credential store (macOS Keychain / Linux Secret Service / Windows Credential Manager) and requires a build with the `secrets-keyring` cargo feature |

### Environment Variables

| Variable | Target Field | Type |
|----------|-------------|------|
| `MIMIR_SECRETS_BACKEND` | `secrets.backend` | `SecretsBackend` (`"file"` / `"keychain"`) |

Invalid values are ignored silently, matching the rest of the env-override layer. A configured `keychain` backend in a build without the `secrets-keyring` feature aborts daemon startup with an actionable error rather than silently falling back to plaintext files.

## GeocoderConfig

Controls the shared OSM Nominatim geocoder injected into the knowledge-graph entity-locations write path (S3 / #193) and the Photos connector (C2 / #196). When disabled, location facts persist with whatever coordinates or address the producer supplied, and the missing half is never filled in (issue #227).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `true` | Master switch; `false` skips geocoder construction at startup |
| `endpoint` | `String` | `"https://nominatim.openstreetmap.org"` | Base URL of the Nominatim instance (no trailing slash); point at a self-hosted instance for heavy use |
| `contact_email` | `Option<String>` | `None` | Contact email appended to the `User-Agent` (recommended for the public instance) |

### Environment Variables

| Variable | Target Field | Type |
|----------|-------------|------|
| `MIMIR_GEOCODER_ENABLED` | `geocoder.enabled` | `bool` |
| `MIMIR_GEOCODER_ENDPOINT` | `geocoder.endpoint` | `String` |
| `MIMIR_GEOCODER_CONTACT_EMAIL` | `geocoder.contact_email` | `String` (empty value clears the field) |

Invalid boolean values are ignored silently, matching the rest of the env-override layer.

## ServerConfig

Controls the daemon's HTTP and Unix socket listeners.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bind_addr` | `String` | `"127.0.0.1:8080"` | TCP bind address for the HTTP server |
| `socket_path` | `Option<String>` | `None` | Path to Unix domain socket for local CLI (disabled by default) |

### Environment Variables

| Variable | Target Field | Type |
|----------|-------------|------|
| `MIMIR_SERVER_BIND_ADDR` | `server.bind_addr` | `String` |
| `MIMIR_SERVER_SOCKET_PATH` | `server.socket_path` | `String` |

When `socket_path` is `None` (default), only the TCP listener is active. On Unix platforms, the recommended default is `~/.local/share/mimir/mimir.sock`, which provides instant daemon detection and filesystem-level access control. See issue #25 for full Unix socket implementation details.

## Database Paths

The daemon opens three SQLite databases. Each path defaults to the shared Mimir data directory (`<data_dir>/context.db`, `<data_dir>/knowledge.db`, `<data_dir>/jobs.db`) but can be overridden independently, mirroring the `context.db_path` pattern. This lets tests isolate every database inside a tempdir and lets multi-instance / dev setups point a single database at an alternate location (issue #233).

| Database | Config field | Env override | Default | Consumer |
|---------|-------------|-------------|---------|---------|
| Context (conversation history) | `context.db_path` | `MIMIR_CONTEXT_DB_PATH` | `<data_dir>/context.db` | `ContextManager` |
| Knowledge graph | `knowledge.db_path` | `MIMIR_KNOWLEDGE_DB_PATH` | `<data_dir>/knowledge.db` | `KnowledgeGraph` |
| Job queue | `scheduler.db_path` | `MIMIR_JOBS_DB_PATH` | `<data_dir>/jobs.db` | `JobQueue` |

When a path is unset the daemon falls back to the corresponding `paths::*_db_path()` resolver. Knowledge-graph backups (`<knowledge_db_path parent>/backups`) are written alongside the knowledge DB so an overridden `knowledge.db_path` keeps backups in the same isolated directory.

## Extending the Configuration

1. Add the new field to the appropriate struct (`LlmConfig`, `AgentConfig`, or `MemoryConfig`).
2. Update the `Default` implementation with a sensible baseline.
3. If the field should be overridable via environment variables, add a line in `Config::apply_env_overrides` following the existing `if let Ok(val)` pattern.
4. Update `config/default.toml` with the new default value.
5. Add a unit test asserting the default and, if applicable, a round-trip test via TOML.
6. Update this document and `docs/wiki/configuration.md`.
