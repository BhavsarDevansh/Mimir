# What Works in Mimir Today

> **Last updated:** 2026-07-08
> **Version:** 0.68.0
> **Release summary:** Phase 2 knowledge-graph work is live — core relationship ontology seeded category-first (Issue #135): predicate aliases for verb canonicalization plus `category_aliases` and category-subtree retrieval for grouping/multi-tag precision; relationship type aliases are the single source of truth for predicate resolution (Issue #133), Fact Ranking & Selection Engine (#108), LLM Condensation Pipeline & Regeneration Triggers (#109), live memory wired into the daemon, the `mimir-knowledge` forgetting system, Agentic Pre-Response Context Retrieval (#128), the Librarian Agent (#130), LLM-orchestrated learning via the `remember` tool (#137), a hardened system prompt that enforces the agentic contract — `retrieve_context` dispatch, no fact invention, and `remember` encouragement (#138), and a redesigned Librarian extraction prompt that injects the same core-facts block as the core agent and learns only from user-labelled messages (#139), and the full pending sensitive-fact confirmation lifecycle — HTTP routes, CLI commands, and a daily auto-cleanup job (#141). v0.57.0 adds the events & reminders subsystem — a lifecycle + recurrence overlay on facts that surfaces upcoming birthdays, appointments, deadlines, and tasks in the Upcoming memory section, with a deterministic scan job and the deprecation of `entity_dates` (#74).

> v0.60.0 adds corroboration detection (#79): when a new non-explicit fact covers the same claim as an existing Active or pending_confirmation fact (same subject + predicate + object, temporally overlapping), Mimir adds a source to the existing fact instead of creating a duplicate, and boosts its confidence +0.05 per independent source (capped at 0.95; explicit and inferred facts excluded). Re-statements from the same source are a no-op, and the confidence change cascades comprehensively to inferred children.

> v0.65.0 adds the shared `normalize_and_insert` ingestion boundary (Phase 3 F4 / #181): the resolve → confidence → sensitivity-gate → insert orchestration is extracted from the conversational `remember` path into one reusable function in `mimir-knowledge::normalize`. Both chat learning and (future) service connectors funnel through it via a provenance-annotated `NormalizedFact` type and a batch-level `Provenance`, so connector-sourced facts get identical confidence scoring, corroboration, supersession, and sensitivity gating — including cross-connector corroboration, where a Gmail flight fact and a Calendar event describing the same trip merge into one knowledge-graph fact with boosted confidence instead of duplicating.

> v0.66.0 adds the full entity-resolution chain (Phase 3 F5 / #182): `resolve_entity` now runs exact name → alias → FTS5 fuzzy (score ≥ 0.9) → create new, restricted to the requested entity type. A short token-overlap query like "John" resolves to the canonical "John Smith" person, while a cross-type fuzzy hit ("Apple" as a concept vs "Apple Inc" the organization) is dropped so a new entity is created instead of a wrong merge. The chain is shared by chat extraction and connectors; alias learning stays explicit via `preferred_name`.

> v0.67.0 defines the runtime `Connector` trait and its data types (Phase 3 F6 / #183): the async, object-safe `Connector` interface every service-ingestion worker implements — `sync` (fetch raw items) → `extract` (produce typed `NormalizedFact`s), plus `authenticate`, `health`, optional `act` write-back, and `forget`. Ingestion is two-step and DB-free: the connector fetches and parses, and the supervisor (F8) will call the shared `normalize_and_insert` pipeline. New types include `ConnectorMode` (polling vs push), `SyncOptions`/`SyncOutcome`, `HealthStatus` (a transient probe, renamed to disambiguate from the persisted lifecycle enums), `ConnectorAction`/`ActionResult`, and `ConnectorError`. No backends sync yet.

> v0.68.0 adds the `ConnectorRegistry` and multi-backend factory dispatch (Phase 3 F7 / #184): the registry maps each `(connector_type, backend)` pair — e.g. `(Email, imap)` or `(Calendar, caldav)` — to a `ConnectorFactory` that constructs the right implementation from a connector's stored config. A connector *type* is the reliability/provenance axis; a *backend* is the provider implementation chosen per instance. New backends register a new factory with no schema change, many backends coexist under one type, and reliability stays per-type. A closure-backed `FnConnectorFactory` and an always-compiled `MockConnectorFactory` keep the registry exercisable under every feature combination. The supervisor, secret store, and concrete backends (Photos, CalDAV Calendar, IMAP Email) land in later Phase 3 issues.


---

## What Is Mimir?

Mimir is a **persistent, personal intelligence** that runs as a local daemon on your machine. It is not a chatbot — it is a stateful companion that remembers facts, preferences, and conversation history across sessions, and becomes more useful the longer you use it.

Key design principles:

- **Local-first** — All data stays on your device. No cloud intermediary.
- **Persistence over ephemerality** — Every interaction is stored, versioned, and retrievable.
- **User sovereignty** — You can inspect, edit, and delete anything Mimir knows.
- **OpenAI-compatible** — Works with any local or remote endpoint that speaks the OpenAI chat completions API.

---

## Architecture at a Glance

Mimir is distributed as a **single binary** (`mimir`) that operates in two modes:

| Mode | Command | What it does |
|------|---------|--------------|
| **Daemon** | `mimir start` | Runs an Axum HTTP server on `127.0.0.1:8080` |
| **Client** | `mimir ask`, `mimir chat`, etc. | Talks to the daemon via HTTP |

Library crates provide code organisation:

- `mimir-core` — LLM client, config, memory, context, personality, tools, skills, paths
- `mimir-server` — Axum routes, state, middleware (library, no binary)
- `mimir-client` — HTTP client for talking to the daemon
- `mimir-api-types` — Shared request/response types
- `mimir-knowledge` — SQLite knowledge graph (Phase 2; wired into daemon via live memory block and condensation pipeline)
- [`Librarian Agent`](../../docs/librarian-agent.md) — On-demand fact-extraction agent; no longer auto-triggered every turn (see #137). Its extraction prompt now reuses the core agent's core-facts block and learns only from `[User]`-labelled messages (#139)

---

## Quick Start

### 1. Build

```bash
cargo build --workspace --release
```

### 2. Initialise

```bash
./target/release/mimir init
```

This creates:
- `~/.config/mimir/config.toml`
- `~/.local/share/mimir/` (data directory)

### 3. Configure

Edit `~/.config/mimir/config.toml`:

```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-4o"
temperature = 0.2

[server]
bind_addr = "127.0.0.1:8080"

[personality]
preset = "transparent"

[memory]
char_limit = 2500
```

Environment variables override config values (e.g. `MIMIR_LLM_API_KEY`).

### 4. Run

```bash
# Terminal 1 — start the daemon
mimir start

# Terminal 2 — ask a question
mimir ask "What is the capital of France?"

# Or start an interactive chat
mimir chat
```

If the daemon is not running, client commands will prompt you to auto-start it.

---

## Feature Reference

### CLI Commands

All client commands talk to the daemon over HTTP. If the daemon is down, you are prompted to start it (unless stdin is not a TTY).

| Command | Status | Description |
|---------|--------|-------------|
| `mimir init` | ✅ Works | First-run bootstrap: creates directories, default config, and optionally installs a systemd user service |
| `mimir start` | ✅ Works | Runs the daemon in the foreground (binds to TCP localhost) |
| `mimir stop` | ✅ Works | Graceful shutdown via POST `/stop` |
| `mimir ask` | ✅ Works | Single-shot query with streaming, piping, model/personality override, incognito mode, and verbose token usage |
| `mimir chat` | ✅ Works | Interactive REPL with session history, `/history` resume, `/memory`, `/status`, `/clear`, `/help`, multi-line input, and SSE streaming |
| `mimir status` | ✅ Works | Health check: config, LLM reachability, queue depth, memory usage |
| `mimir memory` | ✅ Works | Prints the live condensed memory block from the knowledge graph |
| `mimir tool list` | ✅ Works | Lists registered tools and their permissions |
| `mimir tool enable/disable/permission` | ✅ Works | Change tool permission levels (saved to `tools.toml`) |
| `mimir skill list/show/add/delete/enable/disable` | ✅ Works | Manage skills (built-in, user-added, and generated) |
| `mimir kb` | ✅ Works | All `mimir kb` commands route through daemon HTTP; audit and CRUD supported via daemon |

### Chat & Conversation

| Feature | Status | Notes |
|---------|--------|-------|
| Streaming responses | ✅ Works | SSE `/chat/stream` endpoint; tokens arrive in real time |
| Non-streaming responses | ✅ Works | `/chat` endpoint; full response returned as JSON |
| Session persistence | ✅ Works | Each conversation gets a UUID; history is SQLite-backed |
| Session resume | ✅ Works | `/history` in `mimir chat` lets you pick and resume past sessions |
| Context trimming | ✅ Works | Automatically trims to `max_tokens` and `max_turns` config limits |
| Conversation history search (FTS5) | ✅ Works | `search_conversation_history` built-in tool with snippet extraction |
| Incognito mode | ✅ Works | `--incognito` skips all persistence (no session, no memory learning) |
| Model override | ✅ Works | `-m gpt-4o-mini` creates a cached override client |
| Personality override | ✅ Works | `-p concise` overrides the config preset for one query |
| Markdown rendering | ✅ Works | Terminal output adds blank lines around code fences for readability |
| Piped input | ✅ Works | `cat file.txt \| mimir ask …` |
| Multi-line input | ✅ Works | Ctrl-D to submit multi-line text in interactive chat |
| Token usage display | ✅ Works | `--verbose` shows prompt/completion/total token counts |

### Tools & Skills

| Feature | Status | Notes |
|---------|--------|-------|
| Tool registry | ✅ Works | Object-safe `Tool` trait; permissions per tool |
| Skill registry | ✅ Works | Object-safe `Skill` trait with `SkillContext` |
| Builtin tools | ✅ Works | `get_current_time`, `search_web`, `memory`, `context_summary`, etc. |
| Builtin skills | ✅ Works | `research_synthesis`, `test_driven_development` |
| User skills | ✅ Works | Markdown files in `~/.config/mimir/skills/` |
| Generated skills | ✅ Works | Auto-created by the agent; tracked with metrics |
| Metrics tracking | ✅ Works | SQLite-backed invocation counts, success rates, corrections |

### Memory System

| Feature | Status | Notes |
|---------|--------|-------|
| Knowledge graph memory | ✅ Works | Live condensed memory (~2,500 chars) ranked from the knowledge graph and injected into every system prompt |
| LLM-orchestrated learning | ✅ Works | The LLM calls the `remember` tool during conversation to persist facts; learning no longer fires automatically on every turn (#137) |
| Frozen snapshots | ✅ Works | Condensed memory is read from `system_state` once per session; changes don't affect the current chat |
| Knowledge-graph managed | ✅ Works | Manage memory via the knowledge-graph UI/CLI or import/export tools; no memory.md file |
| Size limit enforcement | ✅ Works | Configurable `char_limit` (default 2,500) |

### Configuration

| Feature | Status | Notes |
|---------|--------|-------|
| TOML config file | ✅ Works | `~/.config/mimir/config.toml` |
| Environment overrides | ✅ Works | `MIMIR_LLM_API_KEY`, `MIMIR_BASE_URL`, etc. |
| XDG path resolution | ✅ Works | Respects `XDG_CONFIG_HOME` and `XDG_DATA_HOME` |
| Hot-reload | ✅ Works | Non-sensitive config changes apply without restarting the daemon |
| Auto-initialisation | ✅ Works | First use creates defaults automatically |

### Personality

| Feature | Status | Notes |
|---------|--------|-------|
| Presets | ✅ Works | `transparent`, `concise`, `warm`, `formal` |
| System prompt generation | ✅ Works | Combines preset + condensed memory from the knowledge graph; explicitly marked as non-exhaustive with a note directing the LLM to KG tools |
| CLI override | ✅ Works | `--personality` flag on `mimir ask` |

### Deployment & Operations

| Feature | Status | Notes |
|---------|--------|-------|
| systemd user service | ✅ Works | `mimir init` offers to install and enable it |
| Graceful shutdown | ✅ Works | `mimir stop` or Ctrl-C / SIGTERM |
| Daemon-down detection | ✅ Works | CLI probes `/status`; prompts to start if unreachable |
| Loopback security | ✅ Works | `/stop` is restricted to `127.0.0.1` |
| CORS for local dev | ✅ Works | Whitelisted ports: 8080, 3000, 5173 |

### Knowledge Graph (Phase 2)

| Feature | Status | Notes |
|---------|--------|-------|
| SQLite schema & migrations | ✅ Works | In `mimir-knowledge` crate |
| Entity CRUD | ✅ Works | Types, aliases, deduplication, dates, locations (stubs) |
| Fact CRUD | ✅ Works | Temporal bounds, statuses, dependencies, cascade forget |
| Confidence model | ✅ Works | Graph-derived; no LLM involvement, no decay |
| Inference engine (Rust) | ✅ Works | Transitivity, contradiction, propagation, threshold rules |
| Provenance tracking | ✅ Works | Source tracking with connector_instance_id FK + raw_reference + typed audit log with change_type/changed_by |
| Forgetting system | ✅ Works | Trash, cascade forget, restore, bulk operations |
| FTS5 search | ✅ Works | Full-text search over entities and aliases |
| **Fact extraction pipeline** | ✅ Works | LLM → Rust validation → entity resolution (exact → alias → FTS5 fuzzy ≥ 0.9, type-filtered → create) → confidence → sensitive confirmation → insert (issues #55, #182) |
| **`mimir kb` CLI (daemon-routed)** | ✅ Works | All `mimir kb` commands route through daemon HTTP (no direct DB access); audit and CRUD supported via daemon |
| **Pending sensitive-fact confirmation** | ✅ Works | `GET /kb/pending`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject`; CLI `mimir kb pending|confirm|reject`; optional reject `--reason` written to the audit log (#141) |
| **Pending-fact auto-cleanup** | ✅ Works | Daily `knowledge.pending_cleanup` job hard-deletes facts awaiting confirmation past `retention_days` (default 7); configurable under `[knowledge.pending_cleanup]` (#141) |
| **Relationship type DAG + aliases** | ✅ Works | `relationship_type_hierarchy` and `relationship_type_aliases` tables enable ontology-driven predicate discovery; aliases resolve automatically through `ensure_relationship_type` |
| **Category aliases + subtree retrieval** | ✅ Works | `category_aliases` map domain words (`education`, `hobbies`, `family`…) to Dewey categories; `get_facts_in_category_subtree` gathers facts across a category subtree (#135) |

---

## API Endpoints

The daemon exposes an OpenAI-compatible chat endpoint plus Mimir-specific management endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/status` | Health, config, LLM reachability, memory usage |
| `GET` | `/memory` | Live condensed memory block from the knowledge graph |
| `GET` | `/sessions` | List conversation sessions |
| `GET` | `/sessions/{id}/messages` | Messages for a session (from last compaction) |
| `POST` | `/chat` | Blocking chat with agentic tool loop |
| `POST` | `/chat/stream` | SSE streaming chat |
| `POST` | `/stop` | Graceful shutdown (loopback only) |
| `GET` | `/kb/query` | Query facts for an entity |
| `GET` | `/kb/facts/{id}` | Show a single fact with sources, deps, audit |
| `PATCH` | `/kb/facts/{id}` | Edit mutable fact fields |
| `POST` | `/kb/facts/forget` | Forget facts (single or bulk) |
| `GET` | `/kb/browse` | Graph traversal from an entity |
| `GET` | `/kb/profile` | Generate entity profile from top-confidence facts |
| `GET` | `/kb/audit` | Query the fact audit log |
| `GET` | `/kb/trash` | List trash contents |
| `POST` | `/kb/trash/restore` | Restore facts from trash |
| `DELETE` | `/kb/trash` | Empty trash permanently |
| `GET` | `/kb/pending` | List sensitive facts awaiting confirmation |
| `POST` | `/kb/facts/{id}/confirm` | Confirm a pending fact (→ Active, confidence 1.0) |
| `POST` | `/kb/facts/{id}/reject` | Reject a pending fact (hard-delete + audit; 204) |

---

## Known Limitations & Open Issues

| Issue | Impact | Workaround |
|-------|--------|------------|
| [#71](https://github.com/BhavsarDevansh/Mimir/issues/71) — `mimir chat` streaming bug | Streaming may fail in some environments | Use `mimir ask` for single-shot queries; restart daemon if stream stalls |
| [#45](https://github.com/BhavsarDevansh/Mimir/issues/45) — UTC time | `get_current_time` returns UTC | Ask Mimir to convert to your timezone verbally |
| [#25](https://github.com/BhavsarDevansh/Mimir/issues/25) — Unix socket transport | TCP is the only transport | TCP on `127.0.0.1:8080` is secure for local use |
| | | 

---

## Roadmap Summary

- **Phase 1 — Core Agent** ✅ Complete
- **Phase 2 — Knowledge Graph** ✅ Complete
- **Phase 3 — Connectors** 🚧 In progress — the `mimir-connectors` crate is scaffolded (crate, feature flags `photos`/`calendar`/`gmail`, DB-access boundary via `KnowledgeGraph` only), the `connectors` instance-registry table + `KnowledgeGraph` facade methods landed in #179 / F2 (sync cursor, auth state, and health persist across restarts), the `sources.connector_instance_id` provenance FK migration + per-connector item-count query landed in #180 / F3, the shared `normalize_and_insert` ingestion boundary landed in #181 / F4 (connectors funnel through the same confidence/corroboration/sensitivity pipeline as chat), the full entity-resolution chain landed in #182 / F5, and the runtime `Connector` trait + data types landed in #183 / F6 (the async, object-safe contract every connector implements; two-step DB-free ingestion — `sync` → `extract` → supervisor-owned `normalize_and_insert`), and the `ConnectorRegistry` + multi-backend factory dispatch landed in #184 / F7 (the registry maps `(connector_type, backend)` to a `ConnectorFactory`; new backends register a new factory with no schema change, many backends coexist under one type, and reliability stays per-type). The supervisor, secret store, and concrete backends (calendar, email, file watchers) land in later Phase 3 issues
- **Phase 4 — Reasoning** ⏳ Planned (inference engine expansion)
- **Phase 5 — Proactive Agent** ⏳ Planned (events, reminders, domain surfacing)
- **Phase 6 — Vision** ⏳ Planned (long-term memory consolidation)

See `VISION/09-Roadmap/` for full details.

---

## Getting Help

- Read the per-feature wiki docs in `docs/wiki/` for deep dives on individual subsystems.
- Check the GitHub Issues board for bug reports and feature requests.
- Run `mimir status` to verify daemon health and configuration.
