# Changelog

All notable changes to the Mimir workspace are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-05-22

### Added

- **Tool Registry** (`mimir-core::tools`):
  - `Tool` trait — object-safe async trait for native and CLI tools.
  - `ToolRegistry` — thread-safe registry with `RwLock<HashMap>` for fast lookups.
  - `ToolPermission` — `Auto`, `Ask`, `Disabled` levels.
  - `ToolOutput` — structured result with `to_llm_text()` for compact LLM context.
  - `ToolError` — centralised error covering permissions, timeouts, invalid args, CLI failures.
  - Built-in tools: `GetCurrentTimeTool` (RFC 3339), `EchoTool` (message echo).
  - `CliTool` wrapper — spawns `tokio::process::Command` with absolute-path enforcement, `kill_on_drop`, and configurable timeout.
  - Template argument replacement: `{{key}}` in CLI args replaced from JSON arguments.
  - `tools.toml` configuration at `~/.config/mimir/tools.toml` — array-of-tables for CLI tools, `[permissions]` table for overrides.
  - OpenAI-compatible schema export (`export_openai_tools`) with `type: "function"`, `strict: true`.
- **CLI tool management** (`mimir tool`):
  - `mimir tool list` — table of name, source, permission.
  - `mimir tool enable <name>` — set permission to `Auto`.
  - `mimir tool disable <name>` — set permission to `Disabled`.
  - `mimir tool permission <name> <level>` — explicit permission set.
  - All changes persisted to `tools.toml`.
- Documentation:
  - `docs/tools-registry.md` — technical design and API reference.
  - `docs/wiki/tools.md` — user-facing guide to tools and permissions.

### Changed

- Workspace crate versions bumped from `0.4.0` to `0.5.0`.
- `mimir-core/Cargo.toml` — added `async-trait` and `process` feature to `tokio`.
- `mimir-cli/Cargo.toml` — added `clap` and `tokio` dependencies.
- `mimir-core/src/lib.rs::version()` returns `"0.5.0"`.


## [0.4.0] - 2026-05-22

### Added

- **Personality System** (`mimir-core::personality`):
  - `Personality` — resolves active preset from built-ins and custom files.
  - Four built-in presets: `transparent` (default), `concise`, `warm`, `formal`.
  - Custom preset loading from `~/.config/mimir/personalities/*.personality.md`.
  - Custom presets override built-ins with the same name.
  - Graceful fallback to `transparent` for unknown preset names.
  - `system_prompt(memory_content)` composes preset prompt with persistent memory context.
  - `list_presets()` returns all available preset names.
- **Config extended** (`mimir-core::config`):
  - `PersonalityConfig` with `preset` field (default `"transparent"`).
  - Environment override: `MIMIR_PERSONALITY_PRESET`.
- Documentation:
  - `docs/personality-system.md` — technical design and API reference.
  - `docs/wiki/personality.md` — user-facing guide to personalities.

### Changed

- Workspace crate versions bumped from `0.3.0` to `0.4.0`.
- `config/default.toml` now includes `[personality]` section.
- `mimir-core/src/lib.rs::version()` returns `"0.4.0"`.

## [0.3.0] - 2026-05-21

### Added

- **Conversation Context Manager** (`mimir-core::context`):
  - `ContextManager` — SQLite-backed session and message storage using SQLx.
  - Session lifecycle: `create_session`, `add_user_message`, `add_assistant_message`, `delete_session` (cascades messages).
  - Token-aware trimming: `trim_to_budget` respects `max_turns` (hard) and `max_tokens` (soft).
  - Token usage attribution: `record_usage` attributes completion tokens to the most recent assistant message and prompt delta to the most recent user message.
  - `export_messages` — returns OpenAI-compatible `Vec<Message>` with system prompt always first.
  - `export_conversation` — full audit/log export including cumulative token totals.
  - WAL mode enabled for SQLite; parent directories auto-created.
- **LLM types extended** (`mimir-core::llm::types`):
  - `ChatRequest.stream_options` for `{"include_usage": true}`.
  - `StreamChunk.usage` to capture usage blocks in SSE streams.
  - `StreamItem` enum (`Text` | `Usage`) for usage-aware streaming.
- **LLM client extended** (`mimir-core::llm::client`):
  - `chat_stream_with_usage` — yields `StreamItem` including final usage chunks.
  - `chat_stream` is now a non-breaking wrapper that filters out usage blocks.
- **Config extended** (`mimir-core::config`):
  - `ContextConfig` with `max_tokens`, `max_turns`, and `db_path`.
  - Environment overrides: `MIMIR_CONTEXT_MAX_TOKENS`, `MIMIR_CONTEXT_MAX_TURNS`, `MIMIR_CONTEXT_DB_PATH`.
- Documentation:
  - `docs/context-manager.md` — technical design and API reference.
  - `docs/wiki/context-manager.md` — user-facing guide to conversation context.

### Changed

- Workspace crate versions bumped from `0.2.0` to `0.3.0`.
- `config/default.toml` now includes `[context]` section.

## [0.2.0] - 2026-05-21

### Added

- **memory.md working memory system** (`mimir-core::memory`):
  - `MemoryLoader` — loads `memory.md` from disk or creates a default template.
  - `MemoryManager` — live CRUD operations (`add`, `replace`, `remove`) with character-limit guards and immediate disk persistence.
  - `MemorySnapshot` — frozen per-session clone that preserves LLM prefix-cache performance.
  - Capacity tracking: `current_chars()`, `remaining_chars()`, `usage_pct()`, `is_full()`.
  - Default template includes Identity, Active Projects, Preferences, Temporal, and KB Pointers sections.
- Documentation:
  - `docs/memory-system.md` — technical design and API reference.
  - `docs/wiki/memory.md` — user-facing guide to working memory.
- AGENTS.md rule: semantic version bumps required after every change set.

### Changed

- Workspace crate versions bumped from `0.1.0` to `0.2.0`.
- `tokio` feature flags extended with `fs` in `mimir-core`.

## [0.1.0] - 2026-05-20

### Added

- Initial workspace scaffolding with three crates: `mimir-core`, `mimir-cli`, `mimir-server`.
- Configuration system (`mimir-core::config`) with TOML, env overrides, and defaults.
- LLM client (`mimir-core::llm`) with streaming and non-streaming OpenAI-compatible chat.
