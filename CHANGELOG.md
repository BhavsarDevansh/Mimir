# Changelog

All notable changes to the Mimir workspace are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
