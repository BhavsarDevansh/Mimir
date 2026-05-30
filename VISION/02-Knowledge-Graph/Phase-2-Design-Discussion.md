# Phase 2 Knowledge Graph — Design Discussion Log

> **Purpose:** Capture all architectural decisions, open questions, and design rationale from the Phase 2 planning session so work can be resumed in a fresh thread if the CLI session is interrupted.
> **Last Updated:** 2026-05-30
> **Status:** All decisions (A–K) locked. Ready for issue creation.

---

## Locked Decisions (A–E)

### A. Crate Organization
- **New crate:** `mimir-knowledge` (separate from `mimir-core`).
- **Rationale:** Keeps the knowledge system isolated, testable, and independently versioned. `mimir-core` already handles LLM, config, context, tools, skills — adding the full KG would bloat it.
- **Dependency direction:** `mimir-knowledge` depends on `mimir-core` for shared types (Tool trait, etc.) OR types are shared via `mimir-api-types`. TBD at implementation time.

### B. Database & Storage
- **Backend:** SQLite via `sqlx` (already a workspace dependency).
- **File location:** `~/.local/share/mimir/knowledge.db`.
- **Access model:** Single writer. Only the daemon accesses the database. No concurrent writers, no distributed access.
- **Migration tool:** `sqlx::migrate!()` with ordered `.sql` migration files in `mimir-knowledge/src/db/migrations/`.

### C. Knowledge Graph as a "Second Brain" (3-Layer Architecture)

| Layer | Name | Phase | Description |
|-------|------|-------|-------------|
| 1 | **Tool Retrieval** | Phase 2 | LLM calls `kg_query`, `kg_related`, `kg_search` tools to fetch facts on demand. |
| 2 | **Context Injection** | Phase 2 | Daemon injects a short "recently learned / relevant" fact summary into the system prompt before each chat turn. Explicitly marked as **not exhaustive** so the LLM knows to use tools if it needs more. |
| 3 | **Domain Events / Proactive Surfacing** | Phase 5 | KG emits structured events (`FactInserted`, `ContradictionDetected`, etc.). A future Proactive Agent consumes these to trigger suggestions (e.g., wedding anniversary + food preference → restaurant booking). |

- **Phase 5 issue:** Must be created and tagged as `phase-5` so it slots into the correct roadmap phase.

### D. Inference Engine
- **Approach:** Rust-native rules. Deterministic, compiled, unit-testable.
- **No JSON rules, no LLM-based inference in Phase 2.**
- **V1 rule categories:**
  1. **Transitivity:** `A visited B` + `B is_in C` → `A visited C` (with reduced confidence).
  2. **Contradiction detection:** Two facts with same subject + predicate + overlapping temporal bounds → flag as `Disputed`.
  3. **Confidence propagation:** When a source fact's confidence changes, re-evaluate all downstream inferred facts.
  4. **Threshold rules:** `user rejected_action X` ≥ 3 times → create `preference: reject X`.
- Each rule is a pure function: `(Fact, &KnowledgeGraph) → Option<Vec<Fact>>`.
- Evaluated during fact insertion (for immediate inference) and nightly optimization (for batch inference).

### E. Confidence & Forgetting
- **Confidence is Rust-calculated, never LLM-provided.** If the LLM model changes, confidence logic remains identical.
- **Facts are only forgotten/soft-deleted when:**
  1. Counter-fact exists (fact is `Disputed`), **AND**
  2. Counter-fact's confidence exceeds the original's, **AND**
  3. Dormancy period elapsed (30 days in `Disputed` with no resolution).
- **Never delete** a fact merely because it is old or unverified.
- **Cascading forget:** Explicit dependency tracking via `fact_dependencies` table. When a fact is forgotten:
  1. Soft-delete to `trash` table.
  2. Query all facts whose `fact_dependencies` row references the deleted fact.
  3. Remove the deleted fact from each dependency chain.
  4. If chain is now empty → re-evaluate against remaining graph.
  5. If still derivable → keep with updated chain.
  6. If not derivable → soft-delete the inferred fact too.

---

## Locked Decisions (F–K) — 2026-05-30

### F. Schema Migration Details

**PKs:** All primary keys use `INTEGER PRIMARY KEY AUTOINCREMENT` (not TEXT/UUID). Mimir is single-writer, local-only — no distributed ID need.

**`fact_dependencies` junction table** (replaces `inference_chain TEXT` JSON column):
```sql
CREATE TABLE fact_dependencies (
    parent_fact_id INTEGER NOT NULL REFERENCES facts(id),
    child_fact_id INTEGER NOT NULL REFERENCES facts(id),
    relation_type_id INTEGER NOT NULL REFERENCES relation_types(id),
    PRIMARY KEY (parent_fact_id, child_fact_id, relation_type_id)
);
```

**`relation_types` lookup table** (stable integer IDs, mapped to Rust enum):
```sql
CREATE TABLE relation_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
-- Seed: 1=InferredFrom, 2=Corrects, 3=Supersedes
```

**`fact_statuses` lookup table** (stable integer IDs, mapped to Rust enum):
```sql
CREATE TABLE fact_statuses (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
-- Seed: 1=Active, 2=Inferred, 3=Disputed, 4=Corrected, 5=Superseded, 6=Forgotten
```

**Facts table** uses integer FKs for status:
```sql
CREATE TABLE facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ...
    status_id INTEGER NOT NULL REFERENCES fact_statuses(id) DEFAULT 1,
    ...
);
```

**Rust side:** `FactStatus` and `RelationType` enums derive `sqlx::Type` from the integer directly — no text conversion, no DB join needed in Rust queries. The lookup tables exist purely for DB-level integrity and human readability when inspecting the DB directly.

**Source deduplication:** Composite unique index on `sources(fact_id, source_type, connector_id, raw_reference)`.

**JSON avoidance principle:** JSON columns are reserved only for truly dynamic/untyped data (e.g., connector-specific raw metadata blobs). If a value can be expressed as a Rust struct with known fields at compile time, it goes in a table column or junction table.

### G. Structural Confidence Model

**Core principle: Confidence is derived entirely from graph structure. Zero LLM involvement, zero time-based decay.**

**Initial confidence assigned by Rust based on learning mode:**

| Learning Mode | Initial Confidence | Basis |
|---|---|---|
| Explicit statement | 1.0 | User said it. Immutable barring explicit correction. |
| Connector extraction | 0.70–0.90 | Per-connector reliability score tracked in Rust (e.g., Calendar 0.90, Gmail 0.85). |
| Casual mention | 0.30 | Mentioned in passing. Low but real. |
| Inference | Weighted avg | `avg(source_fact_confidences) × 0.8^depth`. Depth=1 from 0.90 source → 0.72. |

**Confidence changes only when the graph changes:**

| Event | Effect |
|---|---|
| Second independent source corroborates same fact | +0.05 per additional source, capped at 0.95 for non-explicit |
| Counter-fact appears (same subject+predicate, overlapping temporal bounds) | Fact marked `Disputed`. Confidence **unchanged**. |
| Source fact in inference chain forgotten | Recalculate from remaining chain members |
| User explicitly contradicts | Old fact → `Corrected` or `Superseded`. Confidence preserved for provenance. |
| Time passes | **Nothing.** No decay. |

**Per-connector reliability scores:** `HashMap<ConnectorId, f32>` seeded with defaults. Adjusted by feedback loop: user correction → score drops; corroboration by another source → score rises. Simple running average, no ML.

**Temporal awareness for contradiction:** Two facts with same subject+predicate but non-overlapping `valid_from`/`valid_until` ranges are **not** contradictions — they form a timeline. Contradiction detection only fires on overlapping or unbounded temporal ranges. When a new explicit fact arrives with `valid_from = now()`, the old fact gets `valid_until = now()` (marked historical, confidence intact).

**Hard fact replacement:** When explicit replaces explicit (e.g., "green, not blue"), old fact gets `status = Superseded` with original confidence preserved and a `fact_dependencies` row linking old→new with `relation_type = Supersedes`. The old fact was correct at the time — provenance is preserved.

### H. Nightly Optimization Details

**Pass ordering — fixed, serial, each pass feeds the next:**

```
1. Deduplication      → Merged facts change graph for pass 2
  1a. Rust: exact predicate + overlapping temporal → merge (deterministic)
  1b. LLM: semantic near-matches Rust can't catch → flag for merge or review
2. Contradiction Scan → Disputed facts affect inference chain validity
3. Inference Chain    → Recalculate inferred fact confidences from remaining sources
4. Confidence Recalc  → Adjusted confidences may flag dormant facts
5. Dormant Cleanup    → Soft-delete Disputed >30d facts with no resolution
6. Pattern Consolidation → **(Stub in Phase 2)** Logs "not yet implemented", succeeds
7. Compaction         → VACUUM, FTS5 rebuild, ANALYZE (must be last)
```

**LLM-assisted semantic dedup (pass 1b):** Near-matches the Rust pass can't catch (e.g., "visited Rome" vs "trip to Rome" with different predicates, same entity+time). LLM flags candidate pairs. Auto-resolved ones get merged. Uncertain ones go into `dedup_queue` table with `(fact_a, fact_b, suggested_action, llm_confidence)` for human audit via `kb audit`. This builds a corpus for future Rust heuristics.

**Transactional:** Each pass in its own SQLite `BEGIN/COMMIT`. If pass 3 fails, passes 1–2 stay committed. Next night resumes from the failed pass. Prevents single failure from rolling back hours of work.

**Resource budget — configurable in `config.toml`:**
```toml
[knowledge.optimization]
cpu_cores = 1
nice_level = 10
timeout_minutes = 120
schedule_time = "02:00"
```

**Backups:** SQLite `.backup` API to `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db` before pass 1. Keep last 7 daily + last 4 weekly (Sunday). Auto-rotate.

**Trigger:** `JobQueue` infrastructure (shared across all Mimir subsystems). Nightly optimization enqueued as a `System` priority job with `schedule = "02:00"` and `yield_on_user_activity = true`. If daemon is down at scheduled time, runs on next startup if >1h overdue.

**JobQueue design:**
- Priority levels: `System` (nightly optimization, compaction), `Maintenance` (connector syncs), `User` (explicit `kb import`)
- User-activity detection: daemon tracks last interaction timestamp. If `System` job running and chat request arrives, job yields at next pass boundary (between passes, not mid-SQL)
- Yielding pauses the job, commits state marker, resumes when user idle >5 min
- Resource enforcement (nice + CPU affinity) applied at job level
- `kb optimization --status` shows last run results; `kb optimization --run-now` triggers manually

### I. CLI Commands — `mimir kb ...`

**Invocation:** `mimir kb ...` — client-side subcommand talking to daemon via Unix socket/TCP (same pattern as `mimir ask`, `mimir chat`). Daemon gets new Axum routes for KG operations. No direct DB access from CLI.

**Phase 2 commands:**

| Command | Behaviour |
|---|---|
| `kb query "<entity>"` | All facts about entity. Default: colorized table (`tabled` crate). `--json` flag. |
| `kb show <fact-id>` | Single fact detail: sources, dependencies, inference chain. |
| `kb browse --entity "<name>" --depth N` | Graph traversal. Hard limit depth=5, 500 nodes. `--offset`/`--limit` for pagination. |
| `kb edit <fact-id> --field value` | `--confidence`, `--valid-from`, `--valid-until`, `--object`, `--status`. No `$EDITOR` mode. |
| `kb forget <fact-id>` | Single → trash. `--predicate`, `--entity`, `--source`, `--from/--to` for bulk. >100 facts → `--yes`. Sensitive → `--confirm-sensitive`. |
| `kb trash` | List trash with expiry. `kb restore <id>`, `kb restore --all`, `kb trash --empty`. |
| `kb profile [--entity "<name>"]` | Rust-generated bio from top-20 highest-confidence facts. `--json` flag. |
| `kb audit --entity ... --predicate ...` | Disputed facts, pending confirmations, dedup review queue. Interactive resolution. |
| `kb import <path>` | Bulk import (Obsidian/Markdown/CSV). `--dry-run` for preview. |
| `kb export [--format obsidian\|json\|csv]` | Export KG to `~/AgentKnowledge/` or stdout. |
| `kb optimization --status` | Last run, results, next scheduled. `--run-now` for manual trigger. |
| `mimir memory` | Renders condensed memory from KG (see point K). `--json` for raw facts. |

**Phase 3+ deferred:**
| Command | Why |
|---|---|
| `kb heatmap` | Visualization nicety. Needs TUI framework consideration. |
| `kb reset` | Exists as `kb forget --all` in Phase 2. Dedicated scary-confirmation command is polish. |

**Output format principle:** Default = human-readable terminal (colorized, `tabled` crate). Every command supports `--json` for scripting. Confidence color coding: green >0.9, yellow 0.7–0.9, red <0.7.

### J. Testing Strategy

**Test database:** `sqlx::test` with file-backed DB per test (tempdir-backed). FTS5 and WAL need real files — not `:memory:`. Each test owns its tempdir; `sqlx::test` auto-teardown.

**Clock injection:** `Clock` trait with `MockClock` impl. No direct `chrono::Utc::now()` calls from models. All temporal logic testable deterministically.

**Inference unit tests:** Each rule in `inference/rules/` gets isolated tests via `TestGraph` helper that seeds a small DB and runs the rule. Pattern: `graph.seed_fact(...); let result = rule.evaluate(&graph); assert_eq!(result, expected);`.

**DB integration tests:** `mimir-knowledge/tests/` — CRUD, FTS5 search, recursive CTE traversal, temporal queries, confidence recalculation, cascade forgetting.

**Inference engine integration tests:** Seed graph → insert fact → assert side effects. Full cascade: insert → rules fire → inferred facts created → confidence propagated. Contradiction detection with temporal overlap scenarios.

**E2E tests:** Workspace-level `tests/` — "user says X → extraction → storage → query." Partially stubbed until daemon integration (Phase 3), but DB-to-query pipeline fully tested.

**Benchmarks:** `mimir-knowledge/benches/` with Criterion. Seed 10k facts, measure: entity resolution, FTS5 search, graph traversal depth=3, inference chain recalc. Scaffold only — not gating Phase 2.

**Deferred:**
- Property-based testing (`proptest`) — valuable but premature for Phase 2
- Load/concurrency testing — irrelevant for single-writer SQLite
- Connector E2E tests — Phase 3

### K. Migration from `memory.md` to Knowledge Graph

**`memory.md` is removed entirely.** The flat Markdown file no longer exists as a persistent artifact. Memory is rendered on demand via `mimir memory`.

**`mimir memory` command — hybrid Rust + LLM condensation:**

| Layer | Owned by | Role |
|---|---|---|
| Fact selection + ranking | Rust | Deterministic. Which facts qualify + ordering. |
| Structured schema output | Rust | JSON with explicit fields: identity, relationships, preferences, upcoming, general. |
| Condensation | LLM | Compress structured JSON into ≤2500 chars of concise natural language. |
| Validation | Rust | Enforce budget. Fall back to template render if output invalid or > budget. |

**Ranking algorithm (Rust):**
```
score = confidence × category_weight × temporal_boost
```

Category weights: Identity(1.00) > Preferences(0.90) > Relationships(0.85) > Health(0.80) > Upcoming(0.75) > Work/Location(0.60) > General(0.50).

Temporal boost: within 7 days (2.0×), 14 days (1.5×), 30 days (1.2×), beyond (1.0×). Recurring dates (birthdays, anniversaries) detected by matching month+day across years.

**Fill algorithm:** Pull facts ≥0.7 confidence → identity facts always first (~200 chars) → sort remaining by score descending → fill budget greedily → truncate with `…`.

**Regeneration triggers:** Fact inserted/updated/deleted that ranks in top-N for memory inclusion; `mimir memory --refresh`; nightly optimization completes (confidence recalculation may re-rank).

**Storage:** Condensed text stored in `system_state` table (`key = 'condensed_memory'`). Survives daemon restarts. `mimir memory` reads and prints instantly — no computation.

**One-time seeding:** On first KG startup with populated `memory.md` and empty KG:
1. Parse `memory.md` fact-by-fact through extraction pipeline
2. LLM classifies each as explicit/casual, extracts entities
3. Seed KG with parsed facts
4. Rename original to `memory.md.bak-YYYY-MM-DD`
5. Log: "Seeded N facts from legacy memory.md"

**`MemoryManager` refactor (in `mimir-core`):**
- `load_memory()` → queries KG via `mimir-knowledge`, not file
- `save_memory()` → removed (memory is on-demand render, not persisted file)
- Internal API (`get_fact`, `set_fact`, etc.) delegates to KG
- `MemoryManager` becomes a thin facade over KG for Phase 1 compatibility

**Context injection change:** The daemon's system prompt switches from "Here is what I remember about you:" (suggesting completeness) to "Key facts I know about you:" (suggesting curated subset, prompting LLM to use tools for more). The condensed text from `system_state.condensed_memory` is injected directly.

---

## Locked Module Structure (`mimir-knowledge/src/`)

```
mimir-knowledge/src/
├── lib.rs                # Public API: KnowledgeGraph struct + init
├── db/
│   ├── mod.rs            # Connection pool, WAL mode, singleton
│   └── migrations/       # SQLx .sql migration files (001–010+)
├── models/
│   ├── entity.rs         # Entity, EntityType enum
│   ├── fact.rs           # Fact, FactStatus enum
│   ├── source.rs         # Source, SourceType enum
│   ├── preference.rs     # Preference, PreferenceValue
│   └── enums.rs          # FactStatus, RelationType, EntityType (sqlx::Type derivations)
├── queries/
│   ├── entity.rs         # CRUD, alias resolution, dedup
│   ├── fact.rs           # Insert, query, temporal, confidence
│   ├── search.rs         # FTS5 full-text search
│   ├── traverse.rs       # Graph traversal (kg_related)
│   ├── predicate.rs      # Predicate taxonomy DAG queries
│   └── memory.rs         # Memory ranking + condensation pipeline
├── inference/
│   ├── mod.rs            # Rule engine + dispatcher
│   └── rules/            # One file per rule
│       ├── transitivity.rs
│       ├── contradiction.rs
│       └── threshold.rs
├── optimization/
│   ├── mod.rs            # Nightly optimization orchestrator
│   ├── dedup.rs          # Pass 1a: Rust dedup
│   ├── semantic_dedup.rs # Pass 1b: LLM-assisted semantic dedup
│   ├── contradiction.rs  # Pass 2: Contradiction scan
│   ├── inference_chain.rs# Pass 3: Inference chain validation
│   ├── confidence.rs     # Pass 4: Confidence recalculation
│   ├── dormant.rs        # Pass 5: Dormant fact cleanup
│   ├── compaction.rs     # Pass 7: VACUUM, FTS5 rebuild
│   └── backup.rs         # Pre-optimization backup
├── extract.rs            # Fact extraction pipeline (LLM → Rust validation → insert)
└── clock.rs              # Clock trait + RealClock + MockClock
```

---

## Architectural Principles (from discussion)

1. **Keep logic in Rust, not in LLM prompts.** The LLM is used only for: fact extraction (structured schema output), semantic dedup flagging, memory condensation (formatting, not decision-making). All control flow, validation, ranking, and confidence logic is deterministic Rust.
2. **Prefer Rust types over JSON.** If a value can be expressed as a Rust struct with known fields at compile time, it goes in a table column or junction table. JSON columns reserved for truly dynamic data (connector-specific metadata blobs).
3. **Confidence is structural.** Derived from graph topology (number of sources, source type weights, inference chain depth). Never from LLM. Never decays with time. Only changes when the graph changes.
4. **Temporal awareness prevents false contradictions.** Non-overlapping time ranges = timeline, not conflict. Hard fact replacement preserves provenance (old fact was correct at the time).
5. **Single writer.** Daemon is the only process accessing SQLite. CLI commands go through daemon HTTP API. No direct DB access from client.

---

## Next Steps

1. **Create GitHub issues** that break Phase 2 into implementable, sequenced tasks.
2. **Bump workspace version** to `0.20.0` (Phase 2 is a minor feature release).
3. **Create a new branch** `feat/phase-2-knowledge-graph`.
4. **Write failing tests first** (TDD), then implement each issue.
