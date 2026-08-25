# What Works in Mimir Today

> **Last updated:** 2026-08-23
>
> **Version:** 0.141.0
>
> This file is the **feature-level roadmap**: for every feature it records what exists, what is still pending to make it robust, and the GitHub issue tracking each step. The phase-level roadmap lives in `VISION/09-Roadmap/` and the release history in `CHANGELOG.md`; this file deliberately does not repeat either.

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

- `mimir-core` — LLM client + worker pool, config, context, personality, tools, skills, job queue, scheduler, paths
- `mimir-server` — Axum routes, state, middleware (library, no binary)
- `mimir-client` — HTTP client for talking to the daemon
- `mimir-api-types` — Shared request/response types
- `mimir-knowledge` — SQLite knowledge graph: entities, facts, inference, memory, forgetting, optimization, librarian + retrieval agents
- `mimir-connectors` — Connector framework: `Connector` trait, registry, supervisor, secret store, rate limiting, geocoder, and the Photos / CalDAV Calendar / IMAP Email backends

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

This creates `~/.config/mimir/config.toml` and `~/.local/share/mimir/` (data directory), and on Linux offers to install a systemd user service.

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

Status legend: **✅ Works** — implemented and usable today; **🟡 Partial** — works but with known gaps or pending hardening; **❌ Not implemented** — tracked on the roadmap. Every pending item links to its GitHub issue.

### CLI Commands

All client commands talk to the daemon over HTTP except `mimir personality list`, which reads local preset files. If the daemon is down, client commands are prompted to start it (unless stdin is not a TTY).

| Command | Status | Notes & pending work |
|---------|--------|----------------------|
| `mimir init` | ✅ Works | First-run bootstrap: directories, default config, identity prompt, optional systemd install. macOS launchd auto-start is unimplemented ([#285](https://github.com/BhavsarDevansh/Mimir/issues/285)). |
| `mimir start` | ✅ Works | Foreground daemon; binds to the configured `server.bind_addr` (TCP localhost by default). |
| `mimir stop` | ✅ Works | Graceful shutdown via `POST /stop`; verifies the daemon actually exited. |
| `mimir ask` | ✅ Works | Single-shot query with streaming, piping, model/personality override, incognito mode, and verbose token usage. |
| `mimir chat` | ✅ Works | Interactive REPL with `/history` resume, `/memory`, `/status`, `/clear`, `/help`, multi-line input, and SSE streaming. The session id is not persisted across restarts — resuming requires `/history` navigation ([#280](https://github.com/BhavsarDevansh/Mimir/issues/280)). |
| `mimir status` | ✅ Works | Health check: config, LLM reachability, queue depth, memory usage. |
| `mimir memory` | ✅ Works | Prints the live condensed memory block; `--refresh` forces regeneration. |
| `mimir personality list` | ✅ Works | Lists built-in + custom presets with source and optional description ([#387](https://github.com/BhavsarDevansh/Mimir/issues/387)). Runs locally — no daemon needed. |
| `mimir tool list` | ✅ Works | Lists registered tools and their permissions. |
| `mimir tool enable/disable/permission` | ✅ Works | Change tool permission levels (saved to `tools.toml`). |
| `mimir skill list/show/add/delete/enable/disable` | ✅ Works | Manage skills (built-in, user-added). Generated-skill lifecycle is not implemented ([#20](https://github.com/BhavsarDevansh/Mimir/issues/20)). |
| `mimir kb` | ✅ Works | All `mimir kb` commands route through daemon HTTP (no direct DB access); audit, CRUD, trash, pending confirmation, categories, and optimization are supported. The old "migrate kb to daemon routes" issue [#90](https://github.com/BhavsarDevansh/Mimir/issues/90) appears resolved. |
| `mimir connector` | 🟡 Partial | Eleven subcommands (add, auth, catalog, list, status, sync, pause, resume, remove, forget, act) plumbing the daemon routes, including the interactive OAuth PKCE login (A4 / [#205](https://github.com/BhavsarDevansh/Mimir/issues/205)) for `auth.kind=oauth` configs. `mimir connector catalog` lists the daemon's registered `(connector_type, backend)` pairs and `add` pre-flights the requested pair against it before prompting ([#271](https://github.com/BhavsarDevansh/Mimir/issues/271)). The interactive wizard offers email provider presets (Gmail, Outlook/Office 365, Yahoo, Proton Mail Bridge, iCloud, custom IMAP — pre-filling the IMAP defaults and Google/Microsoft OAuth endpoints) and calendar provider presets (Google Calendar, iCloud, Yahoo, custom CalDAV), asks the sync-mode decision (push vs polling with preset/custom intervals) and whether to import existing content, then auto-activates the connector after credential ingest; `list`/`status` show the resolved `push`/`polling` mode ([#397](https://github.com/BhavsarDevansh/Mimir/issues/397)). Non-OAuth credentials can be supplied without leaking to the process list via `--password-stdin`/`--token-stdin` (piped) or `MIMIR_CONNECTOR_PASSWORD`/`MIMIR_CONNECTOR_TOKEN` (env); the `--password`/`--token` flags remain but are visible in `ps`/shell history ([#270](https://github.com/BhavsarDevansh/Mimir/issues/270)). |

### Chat & Conversation

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Streaming responses | ✅ Works | SSE `/chat/stream` endpoint with agentic tool loop; the old streaming bug ([#71](https://github.com/BhavsarDevansh/Mimir/issues/71)) is fixed. |
| Non-streaming responses | ✅ Works | `/chat` endpoint; full response returned as JSON with tool-call info. |
| OpenAI-compatible provider surface | ✅ Works | `GET /v1/models` + `POST /v1/chat/completions` (blocking + streaming) so OpenAI-speaking apps and devices can use Mimir as their LLM provider; the `user` field is a conversation key, model names select personality presets or pass through as upstream overrides, and client tool schemas merge with server tools ([#388](https://github.com/BhavsarDevansh/Mimir/issues/388)). Internal tool activity is not yet surfaced in v1 output ([#464](https://github.com/BhavsarDevansh/Mimir/issues/464)). |
| Session persistence | ✅ Works | Native sessions use numeric `session_id` values (the OpenAI-compatible surface uses a string `user` key as its conversation key instead); history is SQLite-backed and survives restarts. |
| Session resume | ✅ Works | `/history` in `mimir chat` lists sessions and replays messages from the last compaction point. No auto-resume or `--session` flag ([#280](https://github.com/BhavsarDevansh/Mimir/issues/280)). |
| Context trimming | ✅ Works | Drops oldest complete turns (including their tool messages) to `max_tokens` / `max_turns` config limits; system prompt preserved, and the in-flight turn being answered is never trimmed away. |
| Session compaction | ✅ Works | The `session.compaction` hook summarises old turns via the LLM, stores the summary on the session, and deletes the summarised messages; a burst that reaches the hard `max_turns` ceiling is compacted synchronously before trimming so turns are never dropped without a summary, and the summary is shown on `/history` resume and fed back into the model's context ([#279](https://github.com/BhavsarDevansh/Mimir/issues/279), [PR #505](https://github.com/BhavsarDevansh/Mimir/pull/505)). |
| Conversation history search (FTS5) | ✅ Works | `search_conversation_history` built-in tool with BM25 ranking and snippet extraction. |
| Incognito mode | ✅ Works | `--incognito` skips all persistence; write-capable tools are blocked so no facts are stored ([#155](https://github.com/BhavsarDevansh/Mimir/issues/155)). |
| Model override | ✅ Works | `-m gpt-4o-mini` creates a per-request cached override client. |
| Personality override | ✅ Works | `-p concise` overrides the config preset for one query or session. |
| Markdown rendering | ✅ Works | Terminal output adds blank lines around code fences for readability. |
| Piped input | ✅ Works | `cat file.txt \| mimir ask …` |
| Multi-line input | ✅ Works | Trailing `\` continues input on the next line. |
| Token usage display | ✅ Works | `--verbose` shows prompt/completion/total token counts. |

### Tools & Skills

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Tool registry | ✅ Works | Object-safe `Tool` trait; per-tool permissions (auto/ask/disabled) persisted to `tools.toml`; factory-registered tools (e.g. `retrieve_context`) are rebuilt per request with the request-resolved LLM via `ToolContext`, so every tool dispatches through the same path with uniform permission checks ([#441](https://github.com/BhavsarDevansh/Mimir/issues/441)). |
| Skill registry | ✅ Works | Object-safe `Skill` trait with `SkillContext`; built-in, user, and generated origins. |
| Builtin tools | ✅ Works | `get_current_time` (local timezone + UTC offset, [#45](https://github.com/BhavsarDevansh/Mimir/issues/45) fixed), `echo`, `get_weather` (wttr.in, metric-only), `search_conversation_history`; knowledge-graph tools `kg_query`, `kg_related`, `kg_search`, `kg_expand_catalogue`, `kg_facts_in_catalogue`, `retrieve_context`. The `remember` tool was removed in [#386](https://github.com/BhavsarDevansh/Mimir/issues/386) — learning is now the server-side `remember.chat` hook. |
| Builtin skills | ✅ Works | `research_synthesis`, `test_driven_development`. |
| User skills | ✅ Works | Markdown files in `~/.config/mimir/skills/` with YAML frontmatter. |
| Generated skills | ❌ Not implemented | Post-session reflection loop, utility scoring, pruning, and promotion are scaffolded but unused ([#20](https://github.com/BhavsarDevansh/Mimir/issues/20)). |
| Metrics tracking | ✅ Works | SQLite-backed invocation counts, success rates, corrections. |
| Requested tool backlog | ❌ Not implemented | Time-to/since ([#83](https://github.com/BhavsarDevansh/Mimir/issues/83)), web search/scraper/wikipedia ([#93](https://github.com/BhavsarDevansh/Mimir/issues/93)–[#96](https://github.com/BhavsarDevansh/Mimir/issues/96)), RSS ([#97](https://github.com/BhavsarDevansh/Mimir/issues/97)), geocoding tool ([#98](https://github.com/BhavsarDevansh/Mimir/issues/98), deferred wrapper [#192](https://github.com/BhavsarDevansh/Mimir/issues/192)), distance/routing, flights, stocks, sports, weather enhancement, timezone, calculator, curl ([#99](https://github.com/BhavsarDevansh/Mimir/issues/99)–[#106](https://github.com/BhavsarDevansh/Mimir/issues/106)). |

### Memory System

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Knowledge graph memory | ✅ Works | Live condensed memory (~2,500 chars) ranked from the knowledge graph (confidence × category × temporal boost × priority × centrality) and injected into every system prompt. |
| Hook-driven learning | ✅ Works | The `remember.chat` background hook extracts facts after each non-incognito turn, debounced per session and idle-gated ([#386](https://github.com/BhavsarDevansh/Mimir/issues/386)); supersedes the LLM-orchestrated `remember` tool ([#137](https://github.com/BhavsarDevansh/Mimir/issues/137)) and the Librarian fallback ([#156](https://github.com/BhavsarDevansh/Mimir/issues/156)). |
| Frozen snapshots | ✅ Works | Condensed memory is read from `system_state` once per session; changes don't affect the current chat. |
| Knowledge-graph managed | ✅ Works | Memory is a ranked view of the graph; no `memory.md` file. |
| Size limit enforcement | ✅ Works | Configurable `char_limit` (default 2,500). |
| Pinning / deprioritisation | ❌ Not implemented | No way to force a fact into (or out of) the condensed block ([#284](https://github.com/BhavsarDevansh/Mimir/issues/284)). |

### Configuration

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| TOML config file | ✅ Works | `~/.config/mimir/config.toml` with commented defaults. |
| Environment overrides | ✅ Works | `MIMIR_LLM_API_KEY`, `MIMIR_BASE_URL`, etc. |
| XDG path resolution | ✅ Works | Respects `XDG_CONFIG_HOME` and `XDG_DATA_HOME`. |
| Hot-reload | 🟡 Partial | Non-sensitive chat-facing settings (personality, temperature, context limits, tool rounds) apply without restart. Scheduler, job-schedule, and connector settings are read once at startup and silently ignore reloads ([#286](https://github.com/BhavsarDevansh/Mimir/issues/286)). |
| Auto-initialisation | ✅ Works | First use creates defaults automatically. |

### Personality

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Presets | ✅ Works | `transparent`, `concise`, `warm`, `formal`, plus custom `.personality.md` files. |
| System prompt generation | ✅ Works | Preset tone + shared operating directives (honesty, retrieval) + condensed memory block, explicitly marked as a non-exhaustive subset ([#138](https://github.com/BhavsarDevansh/Mimir/issues/138)); learning runs server-side via the `remember.chat` background hook, not a prompt directive. |
| CLI override | ✅ Works | `--personality` flag on `mimir ask` and `mimir chat`. |
| Preset discovery | ✅ Works | `mimir personality list` shows name/source/description ([#387](https://github.com/BhavsarDevansh/Mimir/issues/387)); custom presets can carry an optional `description` in `---` YAML frontmatter; malformed files and unknown configured presets warn instead of silently falling back. |

### Deployment & Operations

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| systemd user service | ✅ Works | `mimir init` offers to install and enable it on Linux. |
| macOS launchd | ❌ Not implemented | `mimir init` prints "planned for a future phase" ([#285](https://github.com/BhavsarDevansh/Mimir/issues/285)). |
| Graceful shutdown | ✅ Works | `mimir stop`, Ctrl-C, or SIGTERM; drains in-flight requests and tears down background tasks; shutdown cause is logged. Runtime teardown never hangs on the config watcher thread, including error paths that drop the runtime before the shutdown broadcast ([#415](https://github.com/BhavsarDevansh/Mimir/issues/415)). |
| Daemon startup (AppState construction) | ✅ Works | Per-subsystem init helpers (`init_context_manager`, `init_tool_registry`, `init_knowledge_graph`, `init_job_queue`, `init_agent_runtime`, `init_scheduler`, `init_connector_framework`) composed in a fixed order by `from_config_with_llm`; issue #281 added the `api_token` field to `AppState` (loaded or generated at startup). |
| Daemon-down detection | ✅ Works | CLI probes `/health`; prompts to auto-start with a 10 s readiness timeout. |
| API authentication | ✅ Works | Every route except `GET /health` requires a bearer token auto-generated at `~/.local/share/mimir/api_token` (mode `0600`); the CLI attaches it automatically, so commands work unmodified ([#281](https://github.com/BhavsarDevansh/Mimir/issues/281)). |
| Loopback security | ✅ Works | Destructive/sensitive routes stay loopback-only as an independent control; the bearer token authenticates every other route, and a non-loopback bind relies on the token alone for authentication and logs a startup warning. |
| CORS for local dev | ✅ Works | Whitelisted ports: 8080, 3000, 5173. |

### Knowledge Graph (Phase 2)

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| SQLite schema & migrations | ✅ Works | In `mimir-knowledge` crate; WAL mode, write-serialisation lock. |
| Entity CRUD | ✅ Works | Types, aliases, deduplication, dates, and locations (the "locations (stubs)" note is stale — the full write path landed in [#193](https://github.com/BhavsarDevansh/Mimir/issues/193)). |
| Fact CRUD | ✅ Works | Temporal bounds, statuses, dependencies, cascade forget. |
| Confidence model | ✅ Works | Graph-derived; no LLM involvement, no decay; corroboration boosts up to 0.95; adjusted per-connector reliability scores flow through every fact-insert path, including the connector pipeline ([#292](https://github.com/BhavsarDevansh/Mimir/issues/292)). |
| Inference engine (Rust) | ✅ Works | Transitivity, contradiction, propagation, threshold rules; deterministic and transparent. |
| Provenance tracking | ✅ Works | Source tracking with `connector_instance_id` FK + `raw_reference` + typed audit log with `change_type`/`changed_by`. |
| Forgetting system | ✅ Works | Trash (30-day retention), cascade forget, restore, bulk operations with safeguards. |
| FTS5 search | ✅ Works | Full-text search over entities and aliases with top-fact retrieval. |
| Fact extraction pipeline | ✅ Works | LLM → Rust validation → entity resolution (exact → alias → FTS5 fuzzy ≥ 0.9, type-filtered → create) → confidence → sensitive confirmation → insert ([#55](https://github.com/BhavsarDevansh/Mimir/issues/55), [#182](https://github.com/BhavsarDevansh/Mimir/issues/182)); comma-separated lists are split into one fact per item for the shared multi-valued predicate set, including the open `favourite_<thing>` family ([#405](https://github.com/BhavsarDevansh/Mimir/issues/405)). |
| `mimir kb` CLI (daemon-routed) | ✅ Works | All commands route through daemon HTTP. |
| Pending sensitive-fact confirmation | ✅ Works | `GET /kb/pending`, confirm/reject routes + CLI; optional reject `--reason` in the audit log ([#141](https://github.com/BhavsarDevansh/Mimir/issues/141)). |
| Pending-fact auto-cleanup | ✅ Works | Daily `knowledge.pending_cleanup` job hard-deletes unconfirmed facts past `retention_days` (default 7). |
| Relationship type DAG + aliases | ✅ Works | `relationship_type_hierarchy` + `relationship_type_aliases`; aliases resolve through `ensure_relationship_type` ([#133](https://github.com/BhavsarDevansh/Mimir/issues/133)); the conversational extraction path enforces the `CANONICAL_PREDICATES` allow-list and rejects invented predicates instead of auto-creating rows ([#401](https://github.com/BhavsarDevansh/Mimir/issues/401)); seeded subject/object entity-type constraints are enforced on every insert path (e.g. `born_on` requires a `DateTime` object) with unconstrained predicates and literal objects exempt ([#402](https://github.com/BhavsarDevansh/Mimir/issues/402)); redundant verbs are consolidated (`based_in`/`lived_in` → `resides_in`, `is_in` → `located_in`) and the DAG is seeded with query-only abstract parents (`employment`, `education`, `residence`, `containment`) for `kg_query --include-subtree` ([#403](https://github.com/BhavsarDevansh/Mimir/issues/403)); every deterministic connector-emitted predicate (`has_event`, `attending`, `took_photo_at`, travel/commerce verbs) is seeded canonical with subject/object constraints where applicable (typed entity-object shapes; literal-object predicates such as `took_photo` stay unconstrained) so a connector sync never auto-creates a predicate row, and the email LLM layer drops non-canonical predicates ([#412](https://github.com/BhavsarDevansh/Mimir/issues/412)). |
| Category aliases + subtree retrieval | ✅ Works | `category_aliases` + `get_facts_in_category_subtree` ([#135](https://github.com/BhavsarDevansh/Mimir/issues/135)). |
| Semantic entity dedup (LLM) | ❌ Not implemented | `enqueue_semantic_dedup` is a stub returning `NotYetImplemented`; the `entity_merge_queue` table and alias-overlap flagging exist ([#282](https://github.com/BhavsarDevansh/Mimir/issues/282)). |
| Pattern consolidation (nightly pass 6) | ❌ Not implemented | Pass logs "not yet implemented" and succeeds ([#67](https://github.com/BhavsarDevansh/Mimir/issues/67)). |
| kb import / export | 🟡 Partial | Obsidian Markdown v1 delivered in v0.148.0 ([#62](https://github.com/BhavsarDevansh/Mimir/issues/62)): `mimir kb export` renders one `.md` file per entity (YAML frontmatter + wiki-links + `Dates`/`Relationships`/`Preferences`/`Facts` sections) and `mimir kb import <path>` parses a vault back through the shared `normalize_and_insert` pipeline with `--dry-run` planning; CSV import/export ([#120](https://github.com/BhavsarDevansh/Mimir/issues/120)) and the bidirectional Obsidian watcher ([#66](https://github.com/BhavsarDevansh/Mimir/issues/66)) remain. |
| kb heatmap / reset polish | ✅ Works | `mimir kb heatmap` renders totals, top entities/predicates, monthly fact distribution, and confidence bands (`--json` for raw data); `mimir kb reset` wipes the graph behind an exact-phrase confirmation, 5-second countdown, and automatic backup ([#69](https://github.com/BhavsarDevansh/Mimir/issues/69)). |
| Entity locations | 🟡 Partial | Write path, geocoding, proximity queries work; sensitive location facts get their overlay when confirmed ([#226](https://github.com/BhavsarDevansh/Mimir/issues/226)); overlapping same-place re-statements are deduplicated instead of duplicating rows ([#228](https://github.com/BhavsarDevansh/Mimir/issues/228)). Geocoder is configurable via the `[geocoder]` section — disable toggle, self-hosted endpoint, contact email ([#227](https://github.com/BhavsarDevansh/Mimir/issues/227)); a flaky batch test is tracked ([#230](https://github.com/BhavsarDevansh/Mimir/issues/230)). |

### Events & Reminders

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Event overlay + scan job | ✅ Works | Lifecycle + recurrence overlay on facts; daily scan derives overlays, auto-completes past reminders, advances recurring events; "Upcoming" memory section ([#74](https://github.com/BhavsarDevansh/Mimir/issues/74)). Superseded facts retire their overlay (dismissed + pending shape dropped) and the scan/render queries exclude superseded/forgotten facts, so a corrected recurring event surfaces exactly once ([#413](https://github.com/BhavsarDevansh/Mimir/issues/413)). |
| Proactive surface | ❌ Not implemented | Notifications, smart completion, and a `mimir events` CLI are Phase 5 work ([#143](https://github.com/BhavsarDevansh/Mimir/issues/143)). |

### Connectors (Phase 3)

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Framework (F1–F13) | ✅ Works | Crate + feature flags, instance registry, provenance FK, shared `normalize_and_insert`, entity-resolution chain, `Connector` trait, registry + factory dispatch, supervised lifecycle, manual sync triggering, secret store, rate-limit/retry primitives, mock connector ([#179](https://github.com/BhavsarDevansh/Mimir/issues/179)–[#190](https://github.com/BhavsarDevansh/Mimir/issues/190)). |
| Daemon wiring + routes (A1–A4) | ✅ Works | Registry/supervisor owned by the daemon, startup restore, CRUD/status/catalog/sync/pause/resume/tokens/actions/forget routes, `mimir connector` CLI, interactive OAuth PKCE login ([#202](https://github.com/BhavsarDevansh/Mimir/issues/202)–[#205](https://github.com/BhavsarDevansh/Mimir/issues/205)). The catalog route exposes every registered `(connector_type, backend)` pair, and `add` pre-flights against it before prompting ([#271](https://github.com/BhavsarDevansh/Mimir/issues/271)). `ConnectorResponse` carries the resolved `push`/`polling` mode derived from the row config ([#397](https://github.com/BhavsarDevansh/Mimir/issues/397)). Enum→wire strings (`connector_type`/`status`/`auth_state`, job priority/run status) come from explicit `as_str()` methods, not `Debug` formatting ([#264](https://github.com/BhavsarDevansh/Mimir/issues/264)). |
| Photos (local) | 🟡 Partial | `notify` watcher, EXIF GPS/datetime, incremental cursor, `took_photo_at`/`visited`/`took_photo` facts, place anchoring, canonical user identity ([#246](https://github.com/BhavsarDevansh/Mimir/issues/246)). Coords-only fallback authors `visited <coords-label>` with the photo path as `raw_reference` provenance ([#250](https://github.com/BhavsarDevansh/Mimir/issues/250)); facts are built through the shared `connector_fact` constructor ([#255](https://github.com/BhavsarDevansh/Mimir/issues/255)); failure-safe cursor adoption — a cycle that fails after scanning re-scans its window on the next cycle instead of skipping it ([#332](https://github.com/BhavsarDevansh/Mimir/issues/332)); RAW formats are deferred. |
| Calendar (CalDAV) | 🟡 Partial | PROPFIND + sync-collection incremental sync, app-password + OAuth refresh, event fact cluster, write-back (`create_event`/`update_event`/`delete_event`), server-side deletion → KB fact lifecycle ([#247](https://github.com/BhavsarDevansh/Mimir/issues/247)), failure-safe sync-token advance — a cycle that fails after fetching re-processes its window on the next cycle instead of skipping it ([#314](https://github.com/BhavsarDevansh/Mimir/issues/314)). Auth error arm duplicated with Email ([#273](https://github.com/BhavsarDevansh/Mimir/issues/273)). |
| Email (IMAP) | 🟡 Partial | `LOGIN`/`XOAUTH2`, `UID FETCH` incremental sync, `IDLE` push with polling fallback, push first-sync backfill (existing mail imported before IDLE; `initial_backfill: false` seeds the cursor to start from "now" — [#397](https://github.com/BhavsarDevansh/Mimir/issues/397)), iMIP invites (REQUEST/REPLY facts keyed by the VEVENT `UID`; `CANCEL` trashes the cancelled event's facts through the #247 tombstone machinery, so it stops surfacing in "Upcoming" — [#283](https://github.com/BhavsarDevansh/Mimir/issues/283)), schema.org JSON-LD, LLM prose extraction with bounded retry + terminal failures, persisted atomically with the sync cursor so retries survive restarts ([#262](https://github.com/BhavsarDevansh/Mimir/issues/262)), failure-safe cursor adoption — a cycle that fails after fetching re-fetches its window on the next cycle instead of skipping it ([#332](https://github.com/BhavsarDevansh/Mimir/issues/332)). Auth error arm duplicated with Calendar ([#273](https://github.com/BhavsarDevansh/Mimir/issues/273)). LLM tool-call parsing is now shared with the conversational path via `mimir-core::llm::parse_tool_output`, and both tool schemas are built once via `LazyLock` ([#259](https://github.com/BhavsarDevansh/Mimir/issues/259)). Email facts are contextualised by the message envelope ([#398](https://github.com/BhavsarDevansh/Mimir/issues/398)): the deterministic spam gate runs before every extraction layer, prose facts are anchored to the email's sent/received dates with a 30-day actionable window (old mail can never become a current action item), and forwarded or misdirected mail is never actionable. The IMAP mail type is the generic `Email` connector type (legacy `gmail` type name and cargo feature renamed, migration 054; the legacy `gmail` wire string stays accepted as an alias), and the wizard provider presets make the Email and Calendar connectors first-class for non-Gmail providers ([#400](https://github.com/BhavsarDevansh/Mimir/issues/400)). |
| OAuth token refresh | ✅ Works | `oauth2` 5.0.0 with `default-features = false` over a custom reqwest 0.13 adapter; redirects disabled, HTTPS/loopback gate, secret-hygiene error mapping ([#240](https://github.com/BhavsarDevansh/Mimir/issues/240)). |
| OAuth PKCE login (A4) | ✅ Works | Interactive loopback flow for the first token: ephemeral loopback listener, browser-opened authorize URL (printed first for headless sessions), CSRF state validation, code exchange, token POST to the daemon ([#205](https://github.com/BhavsarDevansh/Mimir/issues/205)). E2E-tested against an in-process mock OAuth server (HTTPS authorize + HTTP token endpoints, PKCE S256 validation, one-time codes) at both the flow level and the real CLI + daemon level ([#207](https://github.com/BhavsarDevansh/Mimir/issues/207)). |
| OS-keyring secret backend | ✅ Works | Opt-in `secrets-keyring` cargo feature + `secrets.backend = "keychain"` stores connector secrets in the OS credential store (macOS Keychain / Linux or BSD Secret Service / Windows Credential Manager) via `KeyringSecretStore`; a `keychain` config in a build without the feature fails daemon startup loudly ([#188](https://github.com/BhavsarDevansh/Mimir/issues/188)). |
| Mock connector | ✅ Works | Config-driven, gated by the `test-mock-connector` feature; polling/push modes, failure injection, tombstone (`deletions`) reporting ([#190](https://github.com/BhavsarDevansh/Mimir/issues/190), [#247](https://github.com/BhavsarDevansh/Mimir/issues/247)). |
| Rate limiting + retry | ✅ Works | Per-instance GCRA limiter, daily quota, retry/backoff with `Retry-After` honouring ([#189](https://github.com/BhavsarDevansh/Mimir/issues/189)). |
| Geocoder | ✅ Works | OSM Nominatim forward/reverse with rate limiting ([#191](https://github.com/BhavsarDevansh/Mimir/issues/191)); configurable via the `[geocoder]` section — disable toggle, self-hosted endpoint, contact email ([#227](https://github.com/BhavsarDevansh/Mimir/issues/227)). Conversational geocoding tool deferred ([#192](https://github.com/BhavsarDevansh/Mimir/issues/192)). |
| Push-mode manual sync | 🟡 Partial | Manual sync is rejected only for connectors whose mode has *resolved* to push ([#186](https://github.com/BhavsarDevansh/Mimir/issues/186)); an unprobed `auto` connector accepts manual sync as the force-retry until its capability probe proves push ([#475](https://github.com/BhavsarDevansh/Mimir/issues/475)). |
| E2E / integration harness | ✅ Works | Daemon-level tests drive the real CLI + in-process daemon: the full connector lifecycle plus the mock sync→normalize→insert→query round trip asserting `source_type=Connector`, instance provenance, reliability-score confidence, and the corroboration path (second instance boosts +0.05; re-sync is a re-statement no-op) ([#206](https://github.com/BhavsarDevansh/Mimir/issues/206)). The OAuth PKCE login is exercised end-to-end against an in-process mock OAuth server, and the rate-limit/backoff primitives and supervisor edge cases (restore, shutdown cursor persistence, circuit breaker, panic recovery) are covered over real HTTP and real tasks ([#207](https://github.com/BhavsarDevansh/Mimir/issues/207)). The suite is self-contained: daemon-down CLI tests probe the never-bindable loopback port 0, daemon-spawning tests use temp DBs and an injected token, and kill-on-drop guards prevent leaked daemons from locking the real data dir ([#384](https://github.com/BhavsarDevansh/Mimir/issues/384)). |

### Background Jobs & Scheduler

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Job queue + scheduler | ✅ Works | SQLite-backed queue with dedup, debounce, cooldown, idle gating; nightly optimization, pending cleanup, and events scan jobs. Memory condensation runs through the hooks engine instead (issue #386), applying the same dedupe/debounce/cooldown/idle-gate rules per hook. |
| Resource-limit enforcement | ✅ Works | Per-job timeouts, graceful cancellation (daemon shutdown cancels in-flight runs, recorded as `cancelled`), and best-effort CPU affinity / nice / cgroup v2 memory limits wired from `[knowledge.optimization]` ([#91](https://github.com/BhavsarDevansh/Mimir/issues/91)). |

### LLM Client & Worker Pool

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| LLM client | ✅ Works | OpenAI-compatible chat + streaming, retry/backoff on 429/502/503/504, typed error mapping. |
| Worker pool | ✅ Works | Priority queues (user > system) with bounded capacity; connector LLM calls run at system priority. |

### Librarian & Retrieval Agents

| Feature | Status | Notes & pending work |
|---------|--------|----------------------|
| Librarian agent | ✅ Works | On-demand fact extraction from labelled transcripts; registered in the daemon but no longer auto-triggered every turn ([#137](https://github.com/BhavsarDevansh/Mimir/issues/137), [#139](https://github.com/BhavsarDevansh/Mimir/issues/139)). |
| Automatic Librarian fallback | ✅ Superseded | Every non-incognito session gets debounced hook-driven extraction, so no fallback is needed ([#386](https://github.com/BhavsarDevansh/Mimir/issues/386) supersedes [#156](https://github.com/BhavsarDevansh/Mimir/issues/156)). |
| Retrieval agent | ✅ Works | `retrieve_context` dispatches parallel retrieval agents (KG + conversation search, ≤ 25 rounds) ([#128](https://github.com/BhavsarDevansh/Mimir/issues/128)). |

---

## API Endpoints

The daemon exposes an OpenAI-compatible chat endpoint plus Mimir-specific management endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Cheap liveness probe (no LLM or DB access) |
| `GET` | `/status` | Health, config, LLM reachability, memory usage |
| `GET` | `/memory` | Live condensed memory block from the knowledge graph |
| `POST` | `/memory/refresh` | Force memory regeneration (loopback only) |
| `GET` | `/sessions` | List conversation sessions |
| `GET` | `/sessions/{id}/messages` | Messages for a session (from last compaction point) |
| `POST` | `/chat` | Blocking chat with agentic tool loop |
| `POST` | `/chat/stream` | SSE streaming chat |
| `GET` | `/v1/models` | OpenAI-compatible model list (personality presets) |
| `POST` | `/v1/chat/completions` | OpenAI-compatible chat completion (blocking + streaming) |
| `POST` | `/stop` | Graceful shutdown (loopback only) |
| `GET` | `/connectors` | List registered connector instances with derived item counts |
| `POST` | `/connectors` | Register a new connector instance (add-only; validates the backend) |
| `GET` | `/connectors/catalog` | List registered `(connector_type, backend)` pairs the daemon can construct |
| `GET` | `/connectors/{id}` | Show a single connector instance with its item count |
| `DELETE` | `/connectors/{id}` | Stop the runner, delete the instance and its stored credentials (detaches provenance) |
| `POST` | `/connectors/{id}/sync` | Trigger a manual sync |
| `POST` | `/connectors/{id}/pause` | Stop the runner and flip to `Paused` |
| `POST` | `/connectors/{id}/resume` | Re-spawn the runner and flip to `Active` |
| `POST` | `/connectors/{id}/tokens` | Ingest a `SecretBundle` (loopback only) |
| `POST` | `/connectors/{id}/actions` | Dispatch a write-back action |
| `POST` | `/connectors/{id}/forget` | Cascade-trash the connector's facts, delete credentials and row (loopback only) |
| `GET` | `/kb/query` | Query facts for an entity |
| `GET` | `/kb/facts/{id}` | Show a single fact with sources, deps, audit |
| `PATCH` | `/kb/facts/{id}` | Edit mutable fact fields |
| `POST` | `/kb/facts/forget` | Forget facts (single or bulk) |
| `POST` | `/kb/facts/{id}/confirm` | Confirm a pending sensitive fact (loopback only) |
| `POST` | `/kb/facts/{id}/reject` | Reject a pending sensitive fact (loopback only) |
| `GET` | `/kb/pending` | List sensitive facts awaiting confirmation (loopback only) |
| `GET` | `/kb/browse` | Graph traversal from an entity |
| `GET` | `/kb/profile` | Generate entity profile from top-confidence facts |
| `GET` | `/kb/audit` | Query the fact audit log |
| `GET` | `/kb/trash` | List trash contents |
| `POST` | `/kb/trash/restore` | Restore facts from trash |
| `DELETE` | `/kb/trash` | Empty trash permanently |
| `GET` | `/kb/categories` | List categories |
| `POST` | `/kb/categories` | Create a category |
| `GET` | `/kb/categories/{id}` | Show a category with children and fact count |
| `DELETE` | `/kb/categories/{id}` | Delete an empty category |
| `GET` | `/kb/optimization/status` | Optimization job status |
| `POST` | `/kb/optimization/run-now` | Trigger optimization immediately (loopback only) |

---

## Feature-Level Roadmap

The phase-level roadmap lives in `VISION/09-Roadmap/`; this is the per-feature backlog of work needed to make each subsystem robust. Issues are grouped by subsystem; each item is independently actionable.

### Core Agent & Chat

| Work item | Issue |
|-----------|-------|
| `mimir chat` session persistence / auto-resume | [#280](https://github.com/BhavsarDevansh/Mimir/issues/280) |
| Automatic Librarian fallback when `remember` is not called | [#156](https://github.com/BhavsarDevansh/Mimir/issues/156) — superseded by the hooks engine ([#386](https://github.com/BhavsarDevansh/Mimir/issues/386)) |
| Generated skills: reflection loop, utility scoring, pruning | [#20](https://github.com/BhavsarDevansh/Mimir/issues/20) |
| Tool backlog: time-to/since, web, wikipedia, weather, timezone, calculator, curl, sports, stocks, flights, distance, RSS | [#83](https://github.com/BhavsarDevansh/Mimir/issues/83), [#93](https://github.com/BhavsarDevansh/Mimir/issues/93)–[#106](https://github.com/BhavsarDevansh/Mimir/issues/106) |
| Config hot-reload propagation to scheduler/jobs | [#286](https://github.com/BhavsarDevansh/Mimir/issues/286) |
### Knowledge Graph

| Work item | Issue |
|-----------|-------|
| LLM-based semantic entity dedup | [#282](https://github.com/BhavsarDevansh/Mimir/issues/282) |
| Pattern consolidation (nightly pass 6) | [#67](https://github.com/BhavsarDevansh/Mimir/issues/67) |
| kb import / export (CSV; Obsidian Markdown delivered in v0.148.0 via [#62](https://github.com/BhavsarDevansh/Mimir/issues/62)) | [#120](https://github.com/BhavsarDevansh/Mimir/issues/120) |
| Bidirectional Obsidian file watcher | [#66](https://github.com/BhavsarDevansh/Mimir/issues/66) |
| Geocoder configuration (disable, self-hosted, contact email) | [#227](https://github.com/BhavsarDevansh/Mimir/issues/227) |
| Flaky tests: location batch, pending TTL, e2e migration | [#230](https://github.com/BhavsarDevansh/Mimir/issues/230), [#241](https://github.com/BhavsarDevansh/Mimir/issues/241), [#243](https://github.com/BhavsarDevansh/Mimir/issues/243) |
| Memory pinning / deprioritisation | [#284](https://github.com/BhavsarDevansh/Mimir/issues/284) |

### Connectors

| Work item | Issue |
|-----------|-------|
| Connector catalog route + CLI discovery | [#271](https://github.com/BhavsarDevansh/Mimir/issues/271) |
| Secret ingestion via env/stdin (no process-list leak) | [#270](https://github.com/BhavsarDevansh/Mimir/issues/270) |
| DRY: auth error arms, rate-limit default | [#273](https://github.com/BhavsarDevansh/Mimir/issues/273), [#223](https://github.com/BhavsarDevansh/Mimir/issues/223) |
| Geocoding conversational tool | [#192](https://github.com/BhavsarDevansh/Mimir/issues/192) |

### Security & Deployment

| Work item | Issue |
|-----------|-------|
| HTTP API authentication / authorization | [#281](https://github.com/BhavsarDevansh/Mimir/issues/281) |
| Unix domain socket transport (local CLI prefers the socket, TCP fallback) | [#25](https://github.com/BhavsarDevansh/Mimir/issues/25) |
| macOS launchd auto-start | [#285](https://github.com/BhavsarDevansh/Mimir/issues/285) |

### Proactive Agent (Phase 5)

| Work item | Issue |
|-----------|-------|
| Events & reminders: notifications, smart completion, CLI | [#143](https://github.com/BhavsarDevansh/Mimir/issues/143) |
| Domain events / proactive surfacing | [#68](https://github.com/BhavsarDevansh/Mimir/issues/68) |

---

## Known Limitations & Open Issues

| Issue | Impact | Workaround |
|-------|--------|------------|
| [#280](https://github.com/BhavsarDevansh/Mimir/issues/280) — chat session not persisted | Restarting `mimir chat` starts a new session | Use `/history` to resume |
| [#156](https://github.com/BhavsarDevansh/Mimir/issues/156) — no Librarian fallback | Superseded by hook-driven learning ([#386](https://github.com/BhavsarDevansh/Mimir/issues/386)) | None needed |
| [#20](https://github.com/BhavsarDevansh/Mimir/issues/20) — no generated skills | Skills are built-in or hand-written only | Write your own skill files |
| [#143](https://github.com/BhavsarDevansh/Mimir/issues/143) — no proactive notifications | Events surface only in the "Upcoming" memory section | Check `mimir memory` / the Upcoming section |
| [#120](https://github.com/BhavsarDevansh/Mimir/issues/120) — no CSV import/export | Knowledge graph portability covers Obsidian Markdown only (`mimir kb export` / `mimir kb import`, issue #62) | Use `mimir kb export` / `mimir kb import` or the daemon API / CLI CRUD |
| [#270](https://github.com/BhavsarDevansh/Mimir/issues/270) — secret flags leak | `--password`/`--token` appear in the process list and shell history | Use `--password-stdin`/`--token-stdin` (piped) or `MIMIR_CONNECTOR_PASSWORD`/`MIMIR_CONNECTOR_TOKEN` (env) instead |

---

## Roadmap Summary

- **Phase 1 — Core Agent** ✅ Complete (chat, tools, skills, memory, config, personality, deployment)
- **Phase 2 — Knowledge Graph** ✅ Complete (entities, facts, inference, forgetting, memory, librarian, retrieval); hardening backlog in the feature-level roadmap above
- **Phase 3 — Connectors** 🚧 In progress — framework (F1–F13), daemon wiring + CLI (A1–A4, including the interactive OAuth PKCE login), the Photos / CalDAV Calendar / IMAP Email backends, and the mock OAuth server + PKCE/rate-limit/supervisor E2E tests ([#207](https://github.com/BhavsarDevansh/Mimir/issues/207)) are live; the remaining Phase 3 work is the per-backend hardening items listed under Connectors above
- **Phase 4 — Reasoning** ⏳ Planned (inference engine expansion)
- **Phase 5 — Proactive Agent** ⏳ Planned (events, reminders, domain surfacing — [#143](https://github.com/BhavsarDevansh/Mimir/issues/143), [#68](https://github.com/BhavsarDevansh/Mimir/issues/68))
- **Phase 6 — Vision** ⏳ Planned (long-term memory consolidation)

See `VISION/09-Roadmap/` for the phase-level detail.

---

## Getting Help

- Read the per-feature wiki docs in `docs/wiki/` for deep dives on individual subsystems.
- Check the GitHub Issues board for bug reports and feature requests.
- Run `mimir status` to verify daemon health and configuration.
