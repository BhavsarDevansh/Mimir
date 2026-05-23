# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.1] - 2026-05-23

### Fixed

- **Review feedback**: Addressed PR review findings for CLI integration.
  - Removed `colored` from CHANGELOG dependency list (was never added to Cargo.toml).
  - Fixed Markdown lint issues in `docs/cli.md` and `docs/wiki/cli-commands.md` (missing language specifiers, blank lines after headings).
  - `ask.rs`: Persistence failures (ContextManager, session, messages, usage) are now surfaced via `eprintln!` instead of being silently discarded.
  - `chat.rs`: History directory is created via `create_dir_all`; load/save history errors are reported; truncated stream output is no longer persisted.
  - `cli_tests.rs`: All tests now assert `status.success()` where applicable; `test_start_binary_not_found` covers both success and failure branches; `test_ask_piped_input_detection` now properly exercises piped-stdin detection without a query argument.

## [0.8.0] - 2026-05-22

### Added

- **CLI chat subcommands** (`mimir-cli`): direct LLM interaction from the terminal.
  - `mimir start` — spawns `mimir-server` in the background as a detached child.
  - `mimir ask <query>` — single-shot query with optional streaming (`--no-stream`), model override (`--model`), token usage (`--verbose`), incognito mode (`--incognito`), personality override (`--personality`), and piped stdin support.
  - `mimir chat` — interactive REPL with persistent history (`~/.config/mimir/history.txt`), multi-line input, built-in commands (`/exit`, `/clear`, `/memory`, `/status`, `/help`), and conversation context management via `ContextManager`.
  - `mimir status` — displays config path, LLM endpoint/model, connectivity check, and memory.md stats (usage %).
  - `mimir memory` — prints current `memory.md` contents to stdout.
- Dependencies added: `rustyline` (REPL with file history), `is-terminal` (TTY detection), `colored` (terminal colors), `which` (server binary discovery), `futures` (stream processing).

### Changed

- CLI now links `mimir-core` directly for LLM operations; no HTTP round-trip required for CLI commands.
- Version bumped across all workspace crates: `0.7.1` → `0.8.0` (minor: new feature).

## [0.7.1] - 2026-05-22

### Fixed

- **SSE streaming timeout**: Changed `reqwest::Client` from global `timeout(30s)` to `connect_timeout(30s)` so long-lived SSE streams are not prematurely aborted.
- **Queue-full race in `/chat/stream`**: The stream is now enqueued before the 200 SSE response is committed, so `503` is returned immediately when the pool is full instead of a 200 followed by an error event.
- **`assistant_persisted` flag ordering**: Moved `assistant_persisted = true` to after a successful `add_assistant_message` call, so the end-of-stream fallback can still persist the response on failure.
- **Stream drain replaced with drop**: The send-failure path now drops the upstream provider stream immediately (via `drop(stream)`) instead of draining it, allowing faster cancellation.
- **`LlmWorkerPool::new` panic outside Tokio runtime**: Changed constructor to `async fn` so worker tasks are spawned inside a runtime context.
- **`LlmClient` doc comments**: Updated to document async requirements and `connect_timeout` usage.

### Changed

- `LlmClient::new` and `LlmWorkerPool::new` are now `async fn` (breaking change to internal API; acceptable per project policy).

## [0.7.0] - 2026-05-22

### Added

- **HTTP Chat Server** (`mimir-server`): Axum-based HTTP daemon on `127.0.0.1:8080`.
  - `POST /chat` — blocking chat completion with server-managed sessions.
  - `POST /chat/stream` — SSE streaming chat completion with server-managed sessions.
  - `GET /status` — health and runtime state (queue depth, worker count).
  - `GET /memory` — current `memory.md` contents.
  - CORS middleware allowing `localhost` and `127.0.0.1` origins.
  - Unified `ApiError` type with JSON error responses and appropriate HTTP status codes.

- **LLM Worker Pool** (`mimir-core/src/llm/pool.rs`): dual priority queue system.
  - `LlmWorkerPool` with separate bounded user and system queues.
  - Workers always drain user queue before system queue.
  - `LlmError::QueueFull` returned when both queues are at capacity, mapped to HTTP 503 with `Retry-After: 5`.
  - `LlmClient::new()` now creates a default pool with 1 worker.
  - `LlmClient::with_pool()` for test injection.

### Changed

- `LlmClient` refactored to delegate all calls through the worker pool by default.
  - Direct HTTP methods kept as `pub(crate)` for internal worker use.

## [0.6.0] - 2026-05-22

### Added

- **Skill Registry** (`mimir-core/src/skills/`): higher-level workflows registered alongside tools.
  - `Skill` trait with `SkillContext` providing access to `ToolRegistry`, `LlmClient`, and `ContextManager`.
  - `SkillRegistry` supporting built-in, user-added, and system-generated skill origins.
  - `SkillInput` / `SkillOutput` / `SkillError` types mirroring the tool layer.
  - Built-in skills: `research_synthesis` and `test_driven_development`.
  - User skill loading from `~/.config/mimir/skills/*.md` with YAML frontmatter parser (`serde_yaml`).
  - `MarkdownSkill` execution model: body is sent as a system prompt to the LLM with input arguments.
  - SQLite `skill_metrics` table for invocation tracking (`skill_metrics.db`).
  - System-generated skill scaffolding: `SessionSummary` and `should_generate_skill()` trigger detector.
  - CLI commands: `mimir skill list`, `show`, `add`, `delete`, `enable`, `disable`.
  - `SkillRegistry::export_openai_tools()` for OpenAI-compatible function-calling exposure.
