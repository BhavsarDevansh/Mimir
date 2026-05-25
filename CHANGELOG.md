# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
