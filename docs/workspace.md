# Workspace Structure

## Overview

Mimir is organised as a Cargo workspace with one binary crate and five library crates. The workspace root defines shared metadata via `[workspace.package]` so that all members inherit version, authors, licence, and repository information automatically.

## Crate Responsibilities

| Crate | Type | Description |
|-------|------|-------------|
| `mimir` | binary | Single entry point. Dispatches `mimir start` (daemon mode) and all client commands (`ask`, `chat`, `status`, `memory`, `stop`). |
| `mimir-core` | library | Shared domain logic: LLM client, configuration, memory manager, context manager, personality system, tool registry, skill registry, and XDG path resolution. |
| `mimir-server` | library | HTTP server layer built on Axum. Defines routes, application state, middleware, and SSE streaming handlers. No `main.rs`. |
| `mimir-client` | library | Thin HTTP client for talking to the daemon. Wraps `reqwest` and parses SSE streams into `StreamItem` values. |
| `mimir-api-types` | library | Minimal shared serde wire types (`ChatRequest`, `ChatResponse`, `StatusResponse`, `Usage`, `StreamItem`) decoupling the server and client from `mimir-core`. |
| `mimir-knowledge` | library | SQLite-based knowledge graph: entity/fact storage, temporal queries, provenance tracking, and full-text search (Phase 2). |
| `mimir-connectors` | library | Service ingestion framework: background sync workers that fetch external data (email, calendar, photos) and normalize it into knowledge-graph facts. DB access only via the `KnowledgeGraph` facade (no direct `sqlx`). Feature-flagged by backend: `photos`, `calendar`, `gmail`; framework + mock always built. Phase 3 (scaffolded). |

## Metadata Inheritance

The root `Cargo.toml` contains a `[workspace.package]` table with the following fields:

- `version` — single source of truth for all crates
- `authors` — `BhavsarDevansh`
- `license` — `GPL-3.0`
- `description` — generic project description
- `repository` / `homepage` — GitHub URL
- `edition` — `2024`
- `rust-version` — `1.85`

Each member uses `field.workspace = true` to inherit these values. Individual crates may still override `description` with a crate-specific sentence.

## Build Commands

```bash
# Build the entire workspace
cargo build --workspace

# Run all tests (unit, integration, doc-tests)
cargo test --workspace

# Lint everything
cargo clippy --workspace --all-targets --all-features

# Check formatting
cargo fmt -- --check
```

## Version Policy

All workspace members stay in sync on the same semver version unless there is an explicit, documented reason to diverge. The version is bumped in the root `[workspace.package]` table only; members inherit it automatically.

- **PATCH** — backwards-compatible bug fixes, documentation updates
- **MINOR** — backwards-compatible new features, refactors, subsystem additions
- **MAJOR** — breaking changes to public APIs, configuration formats, or data models
