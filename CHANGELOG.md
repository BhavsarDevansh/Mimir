# Changelog

## [0.24.0] - 2026-05-31

### Added

- Fact management subsystem (#50):
  - Schema migration: `predicate TEXT` → `predicate_id INTEGER` FK to `predicates`.
  - `fact_dependencies` FK changed to `ON DELETE RESTRICT` for Rust-orchestrated cascade forget.
  - Full fact CRUD in `mimir-knowledge`: insert, read, update `valid_until`, update status, forget.
  - Temporal overlap logic: Active, Disputed, and open-ended closure handling.
  - Confidence placeholder module (`src/confidence.rs`) with initial values per `SourceType`.
  - Cascade forget with trash retention (30 days) and recursive child evaluation.
  - Audit logging (`fact_audit_log`) for insert, update, status change, and delete.
  - `NewFact` input struct, `Fact::status()` and `Fact::predicate()` helpers.
  - `AuditLogEntry` model and `get_audit_log` query.
  - `KnowledgeGraph` public delegates for all fact operations.
  - Integration tests covering CRUD, temporal timeline, disputed, closure, predicate lookup, audit log, source attachment, cascade forget orphan/survives, trash payload, and confidence values.
  - Technical docs: `docs/fact-management.md`.
  - Wiki docs: `docs/wiki/facts.md`.

# Changelog

## [0.23.2] - 2026-05-31

### Fixed

- Made `KnowledgeGraph` public API consistent: `update_entity`, `insert_entity_date`, and `insert_location` now accept strongly-typed enums (`EntityType`, `EntityDateType`, `RecurrenceType`, `LocationType`) instead of raw `i16` values.
- Wrapped `create_entity` in a transaction with `INSERT ... ON CONFLICT DO NOTHING` and added a DB-level unique expression index on `LOWER(name)` to prevent case-insensitive duplicate races.
- Fixed inverted FTS5 rank filter in `get_by_name` (`rank >= -0.2` → `rank <= -0.2`) and corrected score mapping so more negative (better) bm25 ranks receive higher scores.
- Updated `knowledge-graph-schema.md` to accurately reflect that lookup-table seeding spans migrations `001`, `012`, and `013`.
- Added missing `predicates` and `predicate_constraints` assertions to `all_migrations_apply_cleanly`.

## [0.23.1] - 2026-05-31

### Fixed

- Weekly recurrence in `next_occurrence` computed weekday offset in wrong direction, causing incorrect upcoming dates when current day differed from base weekday.
- `auto_merge_pair` silently deleted `entity_dates` and `entity_locations` via `ON DELETE CASCADE` instead of migrating them to the survivor; now also explicitly removes `preferences` and `entity_merge_queue` rows for the merged entity to prevent FK constraint failures.
- `delete_entity` guard only checked `facts`, allowing raw SQLite FK errors when deleting entities with `preferences` or `entity_merge_queue` entries; now counts all three tables and returns a clean `KnowledgeError`.
- `find_exact_duplicates` performed an O(n²) self-join; rewritten to use a `dup_names` CTE backed by a new expression index on `LOWER(name)`.
- `escape_fts5` only doubled double quotes, leaving `*`, `OR`, `AND`, etc. unescaped; now wraps the query in a quoted phrase and sanitises asterisks to prevent FTS5 syntax errors.

## [0.23.0] - 2026-05-31

### Added

- Entity management subsystem (#49):
  - `DateTime = 8` entity type for temporal nodes.
  - Predicate taxonomy with 10 seeded predicates and type constraints (`validate_predicate`).
  - Full entity CRUD with alias resolution: `create_entity`, `get_by_id`, `get_by_name`, `search`, `update_entity`, `delete_entity`.
  - Alias management: `add_alias`, `remove_alias` with FTS5 index refresh.
  - Entity deduplication: exact-match auto-merge (repoints facts, preserves aliases) and overlapping-alias flagging into `entity_merge_queue`.
  - LLM semantic dedup stub (`enqueue_semantic_dedup`) deferred to Phase 2 (#50+).
  - Entity dates with recurrence resolution: `insert_entity_date`, `get_dates_for_entity`, `get_upcoming_dates`, `delete_entity_date`. Supports None, Daily, Weekly, Monthly, and Yearly (including Feb 29 → Mar 1 fallback).
  - Entity location stubs: `insert_location`, `get_locations`, `update_location`.
  - New `KnowledgeGraph` public API methods delegating to query modules.
  - Integration tests covering CRUD, alias resolution, predicate validation, dates, dedup, and location stubs.

### Changed

- Updated `knowledge-graph-schema.md` and `wiki/knowledge-graph.md` with entity dates, aliases, dedup, and predicate taxonomy documentation.

## [0.22.0] - 2026-05-31

### Added

- `MIMIR_AGENT_MAX_TOOL_ROUNDS` environment variable override
- `max_tool_rounds` entry in default config TOML `[agent]` section
- `StreamItem::SessionId(String)` variant for capturing session IDs from streaming responses
- `event: session_id` SSE event type emitted at stream start
- Regression tests for SSE data-field leading-space preservation and multibyte UTF-8 truncation

### Fixed

- Fixed missing spaces in streaming chat responses caused by `trim_start()` in SSE parser stripping content whitespace instead of only the single SSE-spec space after `data:`
- Fixed `truncate_result` panicking on multibyte UTF-8 by switching from byte-slicing to `chars()`-based truncation
- Fixed agentic tool loop not re-sending tools to LLM after round 0 (both blocking and streaming handlers)
- Fixed streaming `usage_acc` being overwritten each round instead of accumulated across agentic rounds
- Fixed streaming chat not capturing server-assigned `session_id`
- Fixed `Tool::display_name()` overrides being ignored by registry
- Fixed markdown fence blocks in docs missing language identifiers (markdownlint MD040)

## [0.21.1] - 2026-05-30

### Added

- Tool calls are now visible in `mimir chat` and `mimir ask` output, displayed in dim/italic styling (e.g. 🔧 Get Current Time → 2025-05-30T12:00:00Z)
- Agentic tool loop: Mimir can make multiple rounds of tool calls, configurable via `max_tool_rounds` in `[agent]` config (default 100)
- New SSE event type `tool_call` for streaming tool call information to clients
- New `ChatResponse.tool_calls` field for the blocking chat endpoint
- New `ToolCallInfo` type in `mimir-api-types` (name, display_name, result)
- New `display_name()` method on `Tool` trait with automatic Title Case conversion (`snake_to_title_case`)
- New `ToolMetadata.display_name` field populated at registration time
- New `ToolRegistry.get_display_name()` convenience method
- New `AgentConfig.max_tool_rounds` configuration field (default 100)
- Added `colored` crate dependency for terminal styling
- Moved `serde_json` from dev-dependency to dependency in `mimir-api-types`

### Fixed

- Fixed missing spaces in streaming chat responses caused by `trim_start()` in SSE parser stripping content whitespace instead of only the single SSE-spec space after `data:`
- Fixed `truncate_result` panicking on multibyte UTF-8 by switching from byte-slicing to `chars()`-based truncation
- Fixed SSE parser `data:` field handling to strip exactly one leading space per the SSE specification
- Fixed agentic tool loop not re-sending tools to LLM after round 0 (both blocking and streaming)
- Fixed streaming `usage_acc` being overwritten each round instead of accumulated across agentic rounds
- Fixed streaming chat not capturing server-assigned `session_id` — now emitted as `event: session_id` SSE event
- Fixed `Tool::display_name()` overrides being ignored — registry now checks the trait method before falling back to `snake_to_title_case`

### Added

- `MIMIR_AGENT_MAX_TOOL_ROUNDS` environment variable override (e.g. `MIMIR_AGENT_MAX_TOOL_ROUNDS=50`)
- `max_tool_rounds` entry in default config TOML `[agent]` section
- `StreamItem::SessionId(String)` variant for capturing session IDs from streaming responses
- `event: session_id` SSE event type emitted at stream start
- Regression tests for SSE data-field leading-space preservation and multibyte UTF-8 truncation
- Environment variable override test for `MIMIR_AGENT_MAX_TOOL_ROUNDS`

## [0.20.0] - 2026-05-30

### Added

- **mimir-knowledge workspace crate** — SQLite-based knowledge graph foundation:
  - KnowledgeGraph public API with init() and init_with_clock() for deterministic test timestamps.
  - 11 ordered SQLx migrations covering all lookup tables, core tables, queues, trash, audit log, system state, and FTS5 full-text search.
  - 9 lookup tables seeded with stable integer IDs, mapped to Rust enums via #[repr(i16)] discriminants.
  - Clock trait with RealClock and MockClock implementations for testable time.
  - Empty module stubs for future Phase 2 subsystems: queries/, inference/, optimization/, extract.rs.
  - Comprehensive TDD test suite: migration verification, enum roundtrips, DB initialisation, bidirectional enum↔DB sync.
  - Technical documentation (docs/knowledge-graph-schema.md) and user-facing wiki (docs/wiki/knowledge-graph.md).

### Changed

- **Workspace dependencies:** Added [workspace.dependencies] sqlx with migrate feature; mimir-core now references sqlx = { workspace = true }.
- **Paths:** Added knowledge_db_path() to mimir-core::paths resolving to ~/.local/share/mimir/knowledge.db.

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [0.19.3] - 2026-05-27

### Added

- **AGENTS.md: LLM-independence guideline** (project-wide):
  - Added development standard mandating that application logic lives in deterministic Rust code, not in LLM prompts.
  - System prompts must only define role, personality, and high-level goals — never encode conditional logic, parsing rules, or workflow orchestration.
  - Changing the underlying LLM model should never require rewriting application code.
  - Structured outputs, tool schemas, and explicit Rust types must be used for all data crossing the LLM boundary.
- **Memory template redesign** (`mimir-core`):
  - Switched from multi-section templates to a compact free-form scratchpad to simplify memory handling and reduce prompt bloat.
  - The agent now writes compact, self-contained notes with organic grouping rather than rigid section headers.
  - User-facing impact: simpler, less structured LLM memories that are easier to ingest and reason about.
  - See AGENTS.md LLM-independence guideline and VISION/01-Core-Agent/Memory-System.md for design rationale.

## [0.19.2] - 2026-05-27

### Fixed

- **Server killed after exactly 30 seconds** (mimir-server):
  - start_server_with_llm_and_listener wrapped the entire Axum server future in tokio::time::timeout(30s), which forcefully aborted the daemon after 30 seconds regardless of load or health.
  - Removed the mistaken timeout. The server now runs indefinitely until a graceful shutdown is triggered via /stop, Ctrl-C, or SIGTERM. axum::serve().with_graceful_shutdown() already handles graceful termination correctly.

- **Duplicated assistant response in streaming chat** (mimir-server):
  - The SSE streaming endpoint (/chat/stream) sent the full assistant text a second time inside the StreamItem::Usage handler, even when no tool calls had been resolved.
  - The client would then print the complete response twice (once as live chunks, once as the duplicated final block).
  - Fixed by gating the final-text re-send on !tool_calls_acc.is_empty() so it only fires when tool calls were actually resolved and the final text differs from what was already streamed.

## [0.19.1] - 2026-05-27

### Fixed

- **Memory tool missing from ToolRegistry** (`mimir-core`, `mimir-server`, `mimir`):
  - The LLM had no mechanism to persist facts to `memory.md` because the `memory` tool was never registered.
  - Added `MemoryTool` to `mimir-core::tools::builtins` with `add`, `replace`, and `remove` actions backed by `MemoryManager`.
  - Registered `MemoryTool` in daemon `AppState` with the configured `memory.path` and `memory.char_limit`.
  - Registered `MemoryTool` in the CLI tool registry so `mimir tool list` includes it.
  - Updated all built-in personality presets (`transparent`, `concise`, `warm`, `formal`) to instruct the LLM to use the `memory` tool when it learns something about the user that should persist across sessions.
  - Added unit and integration tests covering add, replace, remove, char-limit enforcement, and schema export.

## [0.19.0] - 2026-05-27

### Added

- **Config hot-reload for non-sensitive settings** (`mimir-core`, `mimir-server`, `mimir`):
  - `ReloadableConfig` wrapper in `mimir-core` holds the live `Config` behind an `Arc<tokio::sync::RwLock<Config>>`, with `snapshot()` and `reload()` methods.
  - `ConfigReloadError` enum with `Io`, `Parse`, and `SensitiveFieldChanged` variants for safe reload semantics.
  - Sensitive-field gate: if `llm.endpoint`, `llm.api_key`, `llm.model`, `server.bind_addr`, or `server.socket_path` change on reload, the reload is aborted and the old config is retained.
  - File watcher using `notify` + `notify-debouncer-full` (1-second debounce) watches the config file's parent directory and triggers `ReloadableConfig::reload()` on changes.
  - `SIGHUP` handler on Unix triggers `ReloadableConfig::reload()` for manual hot-reload via `kill -SIGHUP <pid>`.
  - `AppState` now holds an `Arc<ReloadableConfig>` instead of copying individual reloadable fields; routes read via `snapshot().await`.
  - `trim_to_budget()` is now called in the chat handler so that `max_turns` changes take effect.
- **Workspace metadata and README finalization** (all crates):
  - Added `[workspace.package]` table to root `Cargo.toml` with shared `version`, `authors`,
    `license`, `description`, `repository`, `homepage`, `edition`, and `rust-version`.
  - All member `Cargo.toml` files now inherit metadata via `.workspace = true`.
  - Individual crate descriptions added for `mimir`, `mimir-core`, `mimir-server`,
    `mimir-client`, and `mimir-api-types`.

### Changed

- `AppState::from_config` and `AppState::from_config_with_llm` now accept `Arc<ReloadableConfig>` instead of bare `Config`.
- `mimir_server::start_server`, `start_server_with_llm`, `start_server_with_llm_and_listener` all accept `Arc<ReloadableConfig>`.
- `mimir::start::handle_start` wraps the loaded `Config` in `ReloadableConfig` before passing it to the server.
- `mimir-core` `tokio` dependency now includes the `sync` feature for `RwLock`.
- **README.md** updated to reflect Phase 1 reality only:
  - Removed aspirational Phase 2+ architecture descriptions (Knowledge Graph, Connectors,
    Reasoning Engine, Proactive Agent, Vision Tracking).
  - Removed unimplemented Quick Start commands (`mimir connector add`, `mimir kb profile`).
  - Removed Trust Ladder and proactive notification examples.
  - Added Configuration section with config path and environment variable overrides.
- **VISION/09-Roadmap/Phase-1-Core-Agent.md** updated to mark all completed items:
  - All child issues (#30, #31, #32, #33, #34, #35, #36) now checked as complete.
  - Unix domain socket transport (#25) explicitly deferred to Phase 2.
  - Transport section updated to reflect current TCP-only reality with UDS as future work.
- Added `docs/workspace.md` documenting crate responsibilities, metadata inheritance, build commands, and version policy.
- Added `docs/wiki/getting-started.md` with prerequisites, installation, first-run, configuration, quick start, systemd, and troubleshooting.

### Dependencies

- Added `notify = "8.2.0"` and `notify-debouncer-full = "0.7.0"` to `mimir-server`.

## [0.18.0] - 2026-05-27

### Added

- **Conversation history and resumable sessions** (`mimir-core`, `mimir-server`, `mimir-client`, `mimir`):
  - Added `compacted_at` column to the `sessions` table with automatic schema migration for existing databases.
  - `ContextManager::list_sessions()` returns `SessionSummary` rows ordered by `updated_at DESC` with a preview of the latest user message.
  - `ContextManager::get_messages_after_compaction()` returns messages from the last compaction point (or all messages if never compacted).
  - New API types: `SessionSummary`, `ChatMessage`, `SessionMessagesResponse` in `mimir-api-types`.
  - New server endpoints: `GET /sessions` and `GET /sessions/{id}/messages`.
  - New client methods: `MimirClient::sessions()` and `MimirClient::session_messages()`.
  - Chat REPL `/history` command with fuzzy filtering and arrow-key selection via `inquire::Select`.
  - `format_markdown_for_terminal` helper ensures blank lines around Markdown code fences for terminal readability.

### Dependencies

- Added `inquire = "0.9.4"` to the `mimir` binary crate for interactive selection UI.

## [0.17.1] - 2026-05-27

### Fixed

- Addressed review feedback: Markdown formatting, memory path comment accuracy, blank `MIMIR_MEMORY_PATH` handling, `PROBE_CLIENT` graceful error handling, E2E test assertions and hermetic environment.

## [0.17.0] - 2026-05-26

### Added

- End-to-end integration test (`cargo test --test e2e`) that validates the full CLI → daemon → mock LLM round trip.
- `MIMIR_BASE_URL` environment variable override for all CLI commands, cached via `LazyLock`.
- `mimir-server` public API additions: `start_server_with_llm_and_listener`, `start_server_with_llm`, and `AppState::from_config_with_llm` for injecting custom LLM backends.
- `MemoryConfig::path` field and `MIMIR_MEMORY_PATH` environment override for configurable memory file location.
- Static `LazyLock<reqwest::Client>` in daemon guard probe to eliminate per-probe allocations.

### Changed

- `ensure_daemon_running` and `check_daemon_reachable` now accept `&str` instead of `&String`.
- Server log message for bound address now prints the resolved ephemeral port (`listener.local_addr()`) instead of the raw config string.
- All CLI command modules updated to accept a `base_url: &str` parameter.

### Fixed

- `MockProbe::check` in daemon guard tests no longer panics on empty result vector.

## [0.16.3] - 2026-05-25

### Fixed

- **Review feedback addressed** (`mimir-core`, `mimir-server`):
  - Updated CHANGELOG release notes for 0.16.1 to include `test_chat_stream_forwards_tools_to_llm` alongside `test_chat_forwards_tools_to_llm`.
  - Refactored `MockLlmClient` to record chat and stream call messages and tools under a single `Mutex<Vec<CallRecord>>` per path, ensuring atomicity between messages and tools.


## [0.16.2] - 2026-05-25

### Fixed

- **Chat endpoint now handles LLM tool calls** (`mimir-server`, `mimir-core`):
  - Added `ToolCall` and `FunctionCall` structs to `mimir-core::llm::types` for OpenAI-compatible tool call parsing.
  - Extended `Message` with optional `tool_calls` and `tool_call_id` fields, and a custom deserializer that treats `null` content as an empty string (required for assistant messages that contain tool calls instead of text).
  - Added `LlmBackend::chat_message` returning the full assistant `Message` alongside usage; the existing `LlmBackend::chat` method now delegates to it and extracts the text content.
  - Updated `LlmClient`, `LlmWorkerPool`, and `MockLlmClient` to support the new `chat_message` path.
  - `chat_handler` in `mimir-server` now detects when the LLM issues `tool_calls`, executes each tool via `ToolRegistry`, appends the results as `role: tool` messages, and makes a follow-up LLM call to obtain the final assistant response.
  - `chat_stream_handler` in `mimir-server` accumulates tool-call deltas (`StreamItem::ToolCalls`) during SSE streaming, executes the tools when the usage block arrives, makes a follow-up non-streaming LLM call, and streams the final text to the client.
  - Added `StreamItem::ToolCalls` variant and `ToolCallDelta` parsing in `LlmClient::map_sse_event`.
  - Added `test_chat_executes_tool_calls_and_returns_final_response` and `test_chat_stream_executes_tool_calls_and_returns_final_response` verifying the full tool-call loop end-to-end for both blocking and streaming endpoints.

## [0.16.1] - 2026-05-25

### Fixed

- **Chat no longer omits available tools from LLM requests** (`mimir-server`, `mimir-core`):
  - `AppState` now initialises a `ToolRegistry` with built-in tools and loads persisted CLI tool definitions on startup.
  - Both `/chat` and `/chat/stream` handlers forward enabled tools to the LLM backend via the new `ToolRegistry::export_openai_tools_for_llm()` helper.
  - `LlmBackend` trait methods (`chat`, `chat_stream_with_usage`, `chat_stream`) now accept an optional `tools` parameter, threaded through `LlmClient`, `LlmWorkerPool`, and `Job`.
  - `ChatRequest` (internal LLM type) gains a `tools` field with `#[serde(skip_serializing_if = "Option::is_none")]`.
  - `MockLlmClient` extended to record forwarded tools for test assertions.
  - Added `test_chat_forwards_tools_to_llm` and `test_chat_stream_forwards_tools_to_llm` in `mimir-server/src/lib.rs` verifying built-in tools (`get_current_time`, `echo`) are passed to the backend.



## [0.16.0] - 2026-05-25

### Added

- **systemd user service integration** (`mimir-core`, `mimir`):
  - `mimir init` now prompts Linux users to install a systemd user service for auto-start.
  - `generate_service_file()` in `mimir-core` produces a hardened `.service` unit with absolute `ExecStart`, `Restart=on-failure`, `NoNewPrivileges=true`, `ProtectSystem=full`, `ProtectHome=read-only`, `PrivateTmp=true`, and `ReadWritePaths` covering config, data, and cache directories.
  - `install_service_file()` writes the unit to `~/.config/systemd/user/mimir.service`, creating parent directories as needed.
  - `SystemdRunner` async trait with `daemon_reload()` and `enable_now(service)` methods.
  - `RealSystemdRunner` spawns `systemctl` via `tokio::process::Command`.
  - `MockSystemdRunner` records call arguments for unit-test assertions.
  - On `mimir init`, after config and memory setup, Linux users are asked `Install systemd user service for auto-start? [y/N]:`.
    - On **yes**, the service file is generated and installed, then `daemon-reload` and `enable --now mimir` are run. Success prints the `loginctl enable-linger $USER` suggestion.
    - On **no** or EOF, manual `systemctl` instructions are printed.
    - If any `systemctl` command fails, the error is printed and fallback manual instructions are shown; `mimir init` still exits successfully.
  - On **macOS**, a note about future launchd support (Phase 1) is printed.
  - On **Windows**, systemd setup is skipped silently.
- **Path helpers** (`mimir-core`):
  - `systemd_user_dir()` returns `~/.config/systemd/user`.

### Documentation

- `docs/systemd-integration.md`: technical details of service generation, path resolution, `SystemdRunner`, and security hardening.
- `docs/wiki/systemd-setup.md`: user-facing guide on the `mimir init` prompt and manual fallback.
- `docs/cli.md` updated to describe the new systemd prompt under the `init` command.

## [0.15.1] - 2026-05-25

### Fixed

- **Worker spin-loop on dropped watch sender** (`mimir-core`): workers now break their loop when `shutdown_rx.changed()` returns `Err(RecvError)`, preventing a busy spin when the pool is dropped without calling `shutdown()`.
- **Worker shutdown timeout now aborts stuck tasks** (`mimir-core`): `LlmWorkerPool::shutdown()` captures `abort_handle()` before awaiting each worker and calls `abort()` when the 5-second timeout fires, preventing detached tasks from leaking.
- **Non-blocking filesystem sync in shutdown** (`mimir-server`): `AppState::shutdown()` now uses `tokio::fs::OpenOptions` and `tokio::fs::File::sync_all` instead of `std::fs` blocking syscalls inside an async path.
- **Flaky timing-based server test** (`mimir-server`): `test_server_exits_after_stop` now actively polls the TCP port instead of relying on a fixed 500 ms sleep.

## [0.15.0] - 2026-05-25

### Added

- **Graceful daemon shutdown** (`mimir-server`):
  - Server now listens for `SIGINT` and `SIGTERM` (Unix) in addition to the `/stop` HTTP endpoint.
  - `shutdown_signal()` races `tokio::signal::ctrl_c()`, `tokio::signal::unix::signal(SignalKind::terminate())`, and the existing `/stop` watch channel.
  - `axum::serve` is wrapped in a 30-second `tokio::time::timeout`; on timeout a warning is logged and resource cleanup still runs.
  - `AppState::shutdown()` orchestrates cleanup in order:
    1. `ContextManager::close()` flushes and closes the SQLite pool.
    2. `LlmClient::shutdown()` signals the worker pool to exit and drops `reqwest::Client`s.
    3. `memory.md` is synced to disk with `sync_all`.
- **Resource cleanup in `mimir-core`**:
  - `ContextManager::close()` calls `sqlx::SqlitePool::close().await`.
  - `LlmWorkerPool::shutdown()` broadcasts a stop signal to workers, uses `tokio::select!` in the worker loop to break on shutdown, and awaits handles with a 5-second per-handle timeout.
  - `LlmBackend::shutdown()` default no-op trait method so existing mocks are unaffected.
  - `LlmClient::shutdown()` delegates to the pool and drops the HTTP client.
- **`mimir stop` hardened** (`mimir`):
  - When the daemon is unreachable, prints `"Mimir is not running."` to **stderr** and exits with code `1`.
  - After a successful `client.stop()`, waits 2 seconds and probes reachability again.
  - If the daemon is still reachable, prints a warning to stderr and exits with code `1`.
  - If the daemon is no longer reachable, prints `"Mimir daemon stopped."` to stdout and exits `0`.
- **Integration tests**:
  - `test_server_exits_after_stop` (`mimir-server`): spawns `start_server` on a random port, sends `POST /stop`, and asserts the task resolves within 5 seconds.
  - `test_context_manager_close` (`mimir-core`): verifies the SQLite pool is closed and subsequent operations fail.
  - `test_worker_pool_shutdown` (`mimir-core`): verifies pool workers exit cleanly.
  - `test_stop_when_server_down` (`mimir/tests/cli_tests.rs`): asserts exit code `1` and stderr contains `"Mimir is not running."`.

### Changed

- `axum::serve` now uses `app.into_make_service_with_connect_info::<std::net::SocketAddr>()` so the `require_loopback` middleware receives `ConnectInfo` for real TCP connections.

### Dependencies

- Added `"signal"` to `tokio` features in `mimir-server/Cargo.toml`.
- Added `"macros"` to `tokio` features in `mimir-core/Cargo.toml` (required for `tokio::select!`).

## [0.14.1] - 2026-05-25

### Fixed

- **Daemon guard review fixes**:
  - Daemon stdout/stderr are now redirected to null instead of being inherited, preventing log leakage into the client terminal.
  - Poll loop now probes immediately before sleeping, removing unnecessary startup latency.
  - `mimir stop` no longer auto-starts the daemon; it performs a non-interactive reachability probe and prints "daemon already stopped" when the daemon is down.
  - Documentation formatting and wording cleaned up for user-facing guides.

## [0.14.0] - 2026-05-24

### Added

- **`mimir stop` command**: New CLI subcommand that triggers graceful daemon shutdown via the `/stop` HTTP endpoint.
- **Daemon guard (`daemon_guard.rs`)**: Shared `ensure_daemon_running` helper that runs before every client-mode command (`ask`, `chat`, `status`, `memory`, `stop`).
  - Fast-probes `GET /status` with a 500 ms timeout.
  - If the daemon is unreachable, prints `Error: Mimir is not running.` and prompts `Start the server now? [y/N]:`.
  - On user approval (`y`/`yes`), resolves the current executable via `std::env::current_exe()` and spawns it as `mimir start` with inherited stdout/stderr.
  - Polls `/status` with exponential backoff (200 ms → 400 ms → 800 ms → capped at 1 s) until the daemon is ready or a 10 s wall-clock timeout expires.
  - Exactly one auto-start attempt per CLI invocation enforced via a shared `&mut bool` flag.

### Changed

- **Client command handlers** (`ask`, `chat`, `status`, `memory`, `stop`) now call `ensure_daemon_running` before constructing `MimirClient`. If the daemon is down and the user declines the prompt (or stdin is EOF), the command exits cleanly with a clear error message.
- **Integration tests** (`mimir/tests/cli_tests.rs`) updated to account for daemon guard behaviour when no server is running.

## [0.13.0] - 2026-05-24

### Added

- **`mimir-api-types`** (new workspace member): Shared serde wire types (`ChatRequest`, `ChatResponse`, `StatusResponse`, `Usage`, `StreamItem`) decoupling the server and client from `mimir-core`.
- **`mimir-client`** (new workspace member): Thin HTTP client (`MimirClient`) with methods for `chat`, `chat_stream`, `status`, `memory`, and `stop`. Includes a lightweight SSE line parser over `reqwest::bytes_stream()`.
- **Per-request overrides** (`mimir-server`):
  - `model` override via `ChatRequest.model` — `LlmBackend::with_model_override` clones `LlmClient` with the new model.
  - `personality_preset` override via `ChatRequest.personality_preset` — builds a temporary `Personality` for the request.
  - `incognito` mode via `ChatRequest.incognito` — skips all DB persistence and memory learning.
- **Richer `/status` response** (`mimir-server`): Now includes `endpoint`, `model`, `config_path`, `config_exists`, `llm_reachable`, `context_window`, `memory_path`, `memory_exists`, `memory_chars`, `memory_limit`, and `memory_usage_pct`.
- **`/stop` endpoint** (`mimir-server`): POST endpoint that triggers graceful shutdown via a `tokio::sync::watch` channel.

### Changed

- **`mimir` binary chat modules** (`ask`, `chat`, `status`, `memory`) now talk to the daemon via HTTP using `mimir-client` instead of directly importing `mimir-core`. The chat REPL is fully stateless — the daemon owns the session and conversation history.
- **`mimir-server/src/types.rs`** replaced with re-exports from `mimir-api-types`.

## [0.12.2] - 2026-05-24

### Fixed

- Fixed doctest in `mimir-core/src/llm/mock.rs` by adding missing imports and an assertion.

## [0.12.1] - 2026-05-24

### Fixed

- Addressed PR review feedback: black-boxed `create_session` result in
  `context_manager` benchmark for accurate measurement.
- Fixed `docs/wiki/integration-testing.md` command/label mismatch: the "All
  workspace tests" heading now matches the `cargo test --workspace` command
  (removed `--lib` so integration tests are included).
- Synced `mimir` crate version to `0.12.1` to align with `mimir-core` and
  `mimir-server`.

## [0.12.0] - 2026-05-24

### Added

- **`LlmBackend` trait** (`mimir-core/src/llm/backend.rs`): Abstract async trait for
  LLM operations with default implementations for `chat_stream` and introspection
  helpers (`user_queue_depth`, `system_queue_depth`, `worker_threads`,
  `user_queue_has_capacity`).
- **`MockLlmClient`** (`mimir-core/src/llm/mock.rs`): Programmable test double with
  FIFO response queues, call tracking, and builder API. Available under `#[cfg(test)]`
  or via the `mock-llm` Cargo feature.
- **Server integration tests** (`mimir-server/src/lib.rs`): Rewrote test suite to use
  `MockLlmClient` instead of raw TCP mock servers. Added coverage for 500, 503, and
  SSE error events.
- **Wiremock HTTP tests** (`mimir-core/tests/llm_http_integration.rs`): HTTP-level
  integration tests verifying retry logic (429), no-retry logic (400), SSE stream
  parsing, and connection failure handling.
- **Criterion benchmarks** (`mimir-core/benches/`): Four benchmark suites covering
  `ContextManager`, `MemoryManager`, `ToolRegistry`, and `Personality`.

### Changed

- **`AppState.llm_client`** (`mimir-server/src/state.rs`): Changed from
  `Arc<LlmClient>` to `Arc<dyn LlmBackend>` to support test injection and future
  backend implementations.
- **`LlmClient`** (`mimir-core/src/llm/client.rs`): Implements `LlmBackend` with
  straightforward delegation to existing inherent methods.

### Dependencies

- Added `wiremock = "0.6.5"` to `mimir-core` dev-dependencies.
- Added `criterion = { version = "0.8.2", features = ["async_tokio"] }` to
  `mimir-core` dev-dependencies.
- Added `mock-llm` feature flag to `mimir-core`.


