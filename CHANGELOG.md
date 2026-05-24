# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

