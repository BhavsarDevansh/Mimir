# Phase 2: Knowledge Graph

> **Status:** Fully designed. See `VISION/02-Knowledge-Graph/Phase-2-Design-Discussion.md` for all locked decisions.
>
> **Last Updated:** 2026-08-18

## Goal
Build the persistent memory system: entities, facts, temporal reasoning, structural confidence, inference engine, nightly optimization, and user inspection — all backed by a layered SQLite schema with Rust-native logic.

## Duration
4–6 weeks

---

## Deliverables

### 2.1 Database Schema & Crate Setup
- [ ] Create `mimir-knowledge` crate (depends on `mimir-core`)
- [ ] SQLite database at `~/.local/share/mimir/knowledge.db`
- [ ] `sqlx::migrate!()` migration system in `mimir-knowledge/src/db/migrations/`
- [ ] All PKs use `INTEGER PRIMARY KEY AUTOINCREMENT`
- [ ] All enum-like columns use integer FK lookup tables mapped to Rust enums via `sqlx::Type`
- [ ] Tables:
  - `entities` — nodes in the graph
  - `entity_types` — lookup (Person, Place, Event, Object, Concept, Organization, Activity, DateTime)
  - `entity_aliases` — (entity_id, alias) for alias resolution
  - `entity_dates` — temporal properties with recurrence (birthdays, appointments) — **dropped by migration `040`**; superseded by the events overlay (migration `039`)
  - `entity_date_types` — lookup (Birth, Death, Anniversary, Created, Dissolved, Custom) — **dropped by migration `040`**; superseded by the events overlay (migration `039`)
  - `recurrence_types` — lookup (None, Yearly, Monthly, Weekly, Daily)
  - `entity_locations` — geographic properties (schema + stubs for Phase 2)
  - `location_types` — lookup (Home, Work, Visited, Origin, Current, Geographic)
  - `facts` — directed, temporal edges between entities
  - `fact_statuses` — lookup (Active, Inferred, Disputed, Corrected, Superseded, Forgotten)
  - `fact_dependencies` — junction: (parent_fact_id, child_fact_id, relation_type_id)
  - `relation_types` — lookup (InferredFrom, Corrects, Supersedes, Contradicts)
  - `sources` — provenance for every fact
  - `source_types` — lookup (UserEdit, Connector, Inference, Interaction, Import, System)
  - `fact_audit_log` — immutable audit trail of all fact changes
  - `preferences` — user preferences driving agent behavior
  - `preference_categories` — lookup (CalendarBehavior, NotificationStyle, FoodPreference, etc.)
  - `preference_source_types` — lookup (Interaction, Fact, UserEdit)
  - `preference_sources` — junction: (preference_id, source_type_id, source_id)
  - `dedup_queue` — LLM-flagged semantic duplicates for human review
  - `entity_merge_queue` — LLM-flagged entity merges for human review
  - `trash` — soft-deleted facts with expiry
  - `system_state` — key-value store for condensed memory, optimization state
- [ ] FTS5 full-text search on entity names and aliases
- [ ] JSON columns used only for truly dynamic data (`fact_audit_log.old_value/new_value`)

### 2.2 Entity Management
- [ ] CRUD operations for entities
- [ ] Entity type system (lookup table + Rust enum)
- [x] Alias resolution: exact match → alias match → FTS5 fuzzy → create new (Phase 3 F5 / #182)
- [ ] Entity deduplication: Rust exact match (auto-merge) + LLM semantic match (flag for review)
- [ ] Entity merge: re-point all facts → append aliases → soft-delete merged entity
- [x] Events & reminders (recurrence + lifecycle overlay on facts): birthdays, appointments, deadlines with recurrence (migration `039` / #74)
- [ ] Entity locations (schema + stubs): GPS, address — full implementation in Phase 3

### 2.3 Fact Management
- [ ] Insert facts with temporal bounds (`valid_from`, `valid_until`)
- [ ] Query facts by subject/predicate/object
- [ ] Temporal queries ("what was true at time T?")
- [ ] Status management via `fact_statuses` lookup + Rust enum
- [ ] `fact_dependencies` junction table for inference chains, corrections, supersessions
- [ ] Soft-delete to `trash` table with expiry
- [ ] Cascade forget: delete inferred facts whose dependency chains become empty

### 2.4 Structural Confidence Model
- [ ] Confidence derived entirely from graph structure — zero LLM involvement
- [ ] Initial confidence by learning mode: Explicit(1.0), Connector(0.70–0.90 via reliability score), Casual(0.30), Inference(weighted avg × 0.8^depth)
- [ ] Confidence changes only on graph events: corroboration (+0.05/source, capped 0.95), source fact forgotten (recalculate), user contradiction (status change, confidence preserved)
- [ ] No time-based decay
- [ ] Per-connector reliability scores tracked in Rust, adjusted by user corrections
- [ ] Temporal awareness: non-overlapping time ranges = timeline, not contradiction

### 2.5 Provenance & Audit Trail
- [ ] Every fact tracks its source via `sources` table
- [ ] Source types: UserEdit, Connector, Inference, Interaction, Import, System
- [ ] Composite unique index for source deduplication
- [ ] `fact_audit_log` table records every mutation (who, what, when, old/new snapshot)
- [ ] Audit log queryable via `mimir kb audit`

### 2.6 Preference System
- [ ] Separate `preferences` table (not just facts — has unique columns: `overridden_by_user`, behavior-gating semantics)
- [ ] Preference categories lookup table
- [ ] `preference_sources` junction table (replaces JSON `learned_from`)
- [ ] Preference inference from behavior via threshold rules (part of inference engine)
- [ ] Conflict resolution: explicit (1.0, overridden_by_user=1) always wins over inferred

### 2.7 Inference Engine
- [ ] **Rust-native rules** — deterministic, compiled, unit-testable (NOT JSON rules)
- [ ] V1 rules:
  - Transitivity: `A visited B` + `B is_in C` → `A visited C` (reduced confidence)
  - Contradiction detection: same subject+predicate+overlapping temporal bounds → flag Disputed
  - Confidence propagation: when source fact's confidence changes, recalculate downstream
  - Threshold rules: `user rejected_action X` ≥ 3 → create preference
- [ ] Each rule: pure function `(Fact, &KnowledgeGraph) → Option<Vec<Fact>>`
- [ ] Evaluated on fact insertion (immediate) and nightly optimization (batch)

### 2.8 Fact Extraction Pipeline
- [ ] LLM extracts structured facts from user messages (explicit vs casual classification)
- [ ] Rust validates: schema conformance, entity resolution, dedup check
- [ ] Learning mode assigns initial confidence (Explicit=1.0, Casual=0.30)
- [ ] Sensitive facts (health, financial, relationships) require explicit user confirmation
- [ ] Extraction produces structured output — no free-text parsing in Rust

### 2.9 Knowledge Graph Tools (LLM-accessible)
- [ ] `kg_query` — fetch facts by entity
- [ ] `kg_related` — graph traversal (recursive CTE, depth-limited)
- [ ] `kg_search` — FTS5 full-text search across entities and facts
- [ ] Tools registered in `ToolRegistry`, callable by LLM during chat
- [ ] Results formatted as structured data, not natural language (LLM interprets)

### 2.10 Forgetting System
- [ ] Single fact, by predicate, by entity, by source, by time period, full reset
- [ ] Trash bin: soft-delete with 30-day expiry, queryable, restorable
- [ ] Cascade forget: delete inferred facts whose dependency chains become empty
- [ ] Bulk operations (>100 facts) require `--yes`
- [ ] Sensitive category deletions require `--confirm-sensitive`
- [ ] Audit log records all forget operations

### 2.11 Nightly Optimization
- [ ] `JobQueue` infrastructure (shared across all Mimir subsystems)
  - Priority levels: System, Maintenance, User
  - User-activity detection: yield at pass boundaries when user is active
  - Resume when idle >5 minutes
- [ ] 7 serial passes, each in its own transaction:
  1. **Deduplication** — 1a: Rust exact match + 1b: LLM semantic near-match → flag to `dedup_queue`
  2. **Contradiction scan** — overlapping temporal + conflicting predicates → flag Disputed
  3. **Inference chain validation** — recalculate inferred confidences from remaining sources
  4. **Confidence recalculation** — re-score based on current graph state
  5. **Dormant cleanup** — soft-delete Disputed >30d facts with no resolution
  6. **Pattern consolidation** — **(stub in Phase 2)** logs "not yet implemented"
  7. **Compaction** — VACUUM, FTS5 rebuild, ANALYZE
- [ ] Configurable: `cpu_cores`, `nice_level`, `timeout_minutes`, `schedule_time`
- [ ] SQLite `.backup` to `~/.local/share/mimir/backups/` before optimization
- [ ] Keep last 7 daily + 4 weekly backups, auto-rotate
- [ ] Trigger: `tokio::time::interval` at `schedule_time`; if daemon down, run on next startup if >1h overdue
- [ ] `mimir kb optimization --status` and `--run-now` for manual control

### 2.12 Context Injection (Layer 2)
- [ ] System prompt with condensed memory is composed at session creation for non-incognito sessions and reused across turns; incognito requests rebuild it per request
- [ ] Memory block marked "Core facts about the user (condensed subset — not a complete picture; treat as starting context, not exhaustive)" and captured once per session (not exhaustive — LLM knows to use tools)
- [ ] Condensed text stored in `system_state` table, regenerated on KG changes

### 2.13 Librarian Agent (Layer 2 background extraction) — #130
- [x] Generic `Agent` trait and `AgentRuntime` in `mimir-core`
- [x] `LibrarianAgent` in `mimir-knowledge` receives full `ConversationTurn`, user identity, condensed memory, and recent related facts
- [x] Chat route submits `LibrarianGoal` to the runtime after each non-incognito turn
- [ ] Future: goal-directed research agent that constructs `LibrarianGoal`s for specific topics and synthesises findings (deferred to Phase 5 reasoning work)

### 2.14 `mimir memory` Command
- [ ] Renders condensed memory from KG on demand
- [ ] Rust: fact selection + ranking (category weights × confidence × temporal boost × priority × centrality)
- [ ] LLM: condensation into ≤2500 chars natural language
- [ ] Rust: validation + budget enforcement + template fallback
- [ ] `--json` flag for raw ranked facts
- [ ] Recurring dates detected via month+day matching for temporal boost

### 2.15 Migration from `memory.md`
- [x] `memory.md` removed entirely as persistent artifact — landed in v0.37.0 (issue #111), which deleted the file-backed system (`MemoryManager`, `MemoryLoader`, `MemorySnapshot`, `MemoryTool`) and made the Knowledge Graph the sole memory store
- [x] `mimir memory` renders the condensed block on demand from the Knowledge Graph — no file to keep in sync (see `docs/memory-system.md`)
- Note: the originally planned one-time seed (parse legacy file → LLM classify → seed KG → rename `.bak`) and the `MemoryManager` thin-facade refactor never shipped — v0.37.0 deleted the file system outright instead, because memory already flowed through the KG. Those steps are obsolete rather than pending.

### 2.16 CLI Commands (`mimir kb ...`)
- [x] Commands talk to daemon via Unix socket/TCP (same pattern as `mimir ask` — the transport layer from issue #25 covers every CLI command, `kb` included)
- [ ] Daemon exposes new Axum routes for KG operations
- [ ] Phase 2 commands:
  - `kb query "<entity>"` — all facts, colorized table, `--json`
  - `kb show <fact-id>` — single fact detail
  - `kb browse --entity "<name>" --depth N` — graph traversal, depth≤5, 500 nodes max
  - `kb edit <fact-id> --field value` — structured field editing, no `$EDITOR`
  - `kb forget <fact-id>` — single/bulk to trash, `--predicate`, `--entity`, `--source`, `--from/--to`
  - `kb trash` — list, restore, empty
  - `kb profile [--entity "<name>"]` — Rust-generated bio from top-20 facts, `--json`
  - `kb audit --entity ... --predicate ...` — disputed facts, pending confirmations, dedup queue
  - `kb import <path>` — Obsidian Markdown v1 (issue #62), `--dry-run`; CSV deferred
  - `kb export [--dir <path>] [--stdout] [--json]` — Obsidian Markdown v1 to `~/AgentKnowledge/` (issue #62)
  - `kb optimization --status` / `--run-now`
- [ ] Output format: default colorized terminal (`tabled` crate), `--json` on every command
- [ ] Confidence color coding: green >0.9, yellow 0.7–0.9, red <0.7
- [x] Phase 3+ deferred: `kb heatmap`, `kb reset` — delivered in v0.135.0 (issue #69): `mimir kb heatmap` renders totals, top entities/predicates, monthly fact distribution, and confidence bands (`--json` supported); `mimir kb reset` wipes the graph behind an exact-phrase confirmation, 5-second countdown, and automatic backup. See `docs/kb-heatmap-reset.md`.

### 2.17 Obsidian Export & Import
- [x] Export: entities → `.md` files with YAML frontmatter + wiki-links → `~/AgentKnowledge/` — v0.148.0 (issue #62), see `docs/obsidian-export-import.md`
- [x] Import: parse `.md` files → resolve wiki-links → upsert facts (source_type=Import, confidence=0.80) — v0.148.0 (issue #62)
- [x] `entity_id` in YAML frontmatter links back to KG for re-import — v0.148.0 (issue #62)
- [ ] File watcher (bidirectional sync) deferred to Phase 3

### 2.18 Testing
- [ ] `sqlx::test` with file-backed DB per test (tempdir) — FTS5/WAL need real files
- [ ] `Clock` trait with `MockClock` — no direct `chrono::Utc::now()` in models
- [ ] `TestGraph` helper for seeding small DBs in inference rule tests
- [ ] Unit tests: every inference rule, every CRUD operation, alias resolution, dedup
- [ ] DB integration tests (`mimir-knowledge/tests/`): FTS5, recursive CTE, temporal queries, cascade forget
- [ ] Workspace-level E2E tests (`tests/`): extraction → storage → query pipeline
- [ ] Criterion benchmarks (`mimir-knowledge/benches/`): 10k facts, measure entity resolution, FTS5, traversal, inference
- [ ] Property-based testing deferred to Phase 3

### 2.19 Documentation
- [ ] Technical docs in `docs/`: schema, inference engine, confidence model, optimization pipeline
- [ ] Wiki docs in `docs/wiki/`: how the knowledge graph works, CLI usage, best practices

---

## Success Criteria
- [ ] Agent can store and retrieve facts persistently
- [ ] Structural confidence model functions correctly (no LLM, no decay)
- [ ] Inference engine derives new facts from existing ones
- [ ] User can inspect, edit, and forget knowledge via CLI
- [ ] Temporal queries work correctly (timeline vs contradiction)
- [ ] Nightly optimization runs automatically via JobQueue
- [ ] `mimir memory` produces concise, ranked condensation
- [x] Legacy file-backed `memory.md` system removed — v0.37.0 (issue #111) deleted it outright and the Knowledge Graph became the sole memory store; no data migration ran and memory renders from the graph
- [x] Obsidian export functional — v0.148.0 (issue #62); import is the manual counterpart (`mimir kb import <path>`)
- [ ] All tests pass, benchmarks show acceptable performance with 10k+ facts

## Dependencies
- Phase 1 (Core Agent)

## Deferred to Later Phases
| Item | Phase | Issue Tag |
|---|---|---|
| Entity locations full implementation (GPS, proximity queries) | 3 | `phase-3` |
| File watcher for bidirectional Obsidian sync | 3 | `phase-3` |
| Pattern consolidation in nightly optimization | 3 | `phase-3` |
| Domain events / proactive surfacing | 5 | `phase-5` |
| Property-based testing | 3 | `phase-3` |
| Connector-specific E2E tests | 3 | `phase-3` |
