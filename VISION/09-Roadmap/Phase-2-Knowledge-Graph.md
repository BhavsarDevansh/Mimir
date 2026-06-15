# Phase 2: Knowledge Graph

> **Status:** Fully designed. See `VISION/02-Knowledge-Graph/Phase-2-Design-Discussion.md` for all locked decisions.
> **Last Updated:** 2026-05-30

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
  - `entity_types` — lookup (Person, Place, Event, Object, Concept, Organization, DateTime)
  - `entity_aliases` — (entity_id, alias) for alias resolution
  - `entity_dates` — temporal properties with recurrence (birthdays, appointments)
  - `entity_date_types` — lookup (Birthday, Anniversary, Appointment, Deadline, RecurringEvent)
  - `recurrence_types` — lookup (None, Yearly, Monthly, Weekly, Daily)
  - `entity_locations` — geographic properties (schema + stubs for Phase 2)
  - `location_types` — lookup (Home, Work, Previous, Frequent, EventLocation)
  - `facts` — directed, temporal edges between entities
  - `fact_statuses` — lookup (Active, Inferred, Disputed, Corrected, Superseded, Forgotten)
  - `fact_dependencies` — junction: (parent_fact_id, child_fact_id, relation_type_id)
  - `relation_types` — lookup (InferredFrom, Corrects, Supersedes)
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
- [ ] Alias resolution: exact match → alias match → FTS5 fuzzy → create new
- [ ] Entity deduplication: Rust exact match (auto-merge) + LLM semantic match (flag for review)
- [ ] Entity merge: re-point all facts → append aliases → soft-delete merged entity
- [ ] Entity dates (full implementation): birthdays, appointments, deadlines with recurrence
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
- [ ] Daemon injects condensed memory into system prompt before each chat turn
- [ ] Marked as "Key facts I know about you:" (not exhaustive — LLM knows to use tools)
- [ ] Condensed text stored in `system_state` table, regenerated on KG changes

### 2.13 Librarian Agent (Layer 2 background extraction) — #130
- [x] Generic `Agent` trait and `AgentRuntime` in `mimir-core`
- [x] `LibrarianAgent` in `mimir-knowledge` receives full `ConversationTurn`, user identity, condensed memory, and recent related facts
- [x] Chat route submits `LibrarianGoal` to the runtime after each non-incognito turn
- [ ] Future: goal-directed research agent that constructs `LibrarianGoal`s for specific topics and synthesises findings (deferred to Phase 5 reasoning work)

### 2.14 `mimir memory` Command
- [ ] Renders condensed memory from KG on demand
- [ ] Rust: fact selection + ranking (category weights × confidence × temporal boost)
- [ ] LLM: condensation into ≤2500 chars natural language
- [ ] Rust: validation + budget enforcement + template fallback
- [ ] `--json` flag for raw ranked facts
- [ ] Recurring dates detected via month+day matching for temporal boost

### 2.15 Migration from `memory.md`
- [ ] `memory.md` removed entirely as persistent artifact
- [ ] One-time seed: parse legacy `memory.md` → classify via LLM → seed KG → rename to `.bak`
- [ ] `MemoryManager` refactored: `load_memory()` queries KG, `save_memory()` removed
- [ ] `MemoryManager` becomes thin facade over KG for Phase 1 compatibility

### 2.16 CLI Commands (`mimir kb ...`)
- [ ] Commands talk to daemon via Unix socket/TCP (same pattern as `mimir ask`)
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
  - `kb import <path>` — Obsidian/Markdown/CSV, `--dry-run`
  - `kb export [--format obsidian|json|csv]` — to `~/AgentKnowledge/` or stdout
  - `kb optimization --status` / `--run-now`
- [ ] Output format: default colorized terminal (`tabled` crate), `--json` on every command
- [ ] Confidence color coding: green >0.9, yellow 0.7–0.9, red <0.7
- [ ] Phase 3+ deferred: `kb heatmap`, `kb reset`

### 2.17 Obsidian Export & Import
- [ ] Export: entities → `.md` files with YAML frontmatter + wiki-links → `~/AgentKnowledge/`
- [ ] Import: parse `.md` files → resolve wiki-links → upsert facts (source_type=Import, confidence=0.80)
- [ ] `entity_id` in YAML frontmatter links back to KG for re-import
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
- [ ] Legacy `memory.md` successfully migrated
- [ ] Obsidian export functional
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
| `kb heatmap` command | 3+ | `phase-3` |
| `kb reset` command | 3+ | `phase-3` |
| Property-based testing | 3 | `phase-3` |
| Connector-specific E2E tests | 3 | `phase-3` |
