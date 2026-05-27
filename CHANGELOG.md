# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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


