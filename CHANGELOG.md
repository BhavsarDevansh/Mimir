# Changelog

## [0.41.3] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (additional finding addressed):
  - Added `MIMIR_SCHEDULER_DEBOUNCE_SECONDS` and `MIMIR_SCHEDULER_COOLDOWN_SECONDS` environment variable overrides in `mimir-core/src/config.rs`, following the existing `apply_env_overrides_with` pattern.

## [0.41.2] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (5 findings addressed):
  - Removed unused `jq_for_opt` clone in `mimir-server/src/state.rs` optimisation job closure.
  - Added `DaemonJob::from_job_id()` helper to eliminate duplicated string-to-variant mapping in `mimir-core/src/scheduler.rs`.
  - Log SQL errors in `relationship_type_id` instead of silently swallowing them with `.ok()?`.
  - Clarified memory condensation documentation: separated 2500-character budget from top-N limit (500).
  - Corrected nightly-optimization wiki to state "last minute" instead of "last few minutes" to match the 60-second cooldown default.

## [0.41.1] — 2026-06-11

### Fixed

- `LlmWorkerPool` `in_flight` counter is now incremented and decremented around every job processed by workers. Previously the counter was always zero, causing the scheduler's idle gate to incorrectly allow background jobs while LLM requests were in flight.
- `BackgroundScheduler::submit()` now correctly deduplicates against jobs that are already *running*, not just pending. Prevents back-to-back execution when a submit arrives during an active run.
- `BackgroundScheduler::shutdown()` is now called during `AppState::shutdown()`, wiring the scheduler's private shutdown channel into the daemon's graceful teardown sequence. Prevents stale "Running" DB rows when the runtime drops mid-job.

## [0.41.0] — 2026-06-11

### Added

- Unified `BackgroundScheduler` in `mimir-core` that deduplicates, debounces, and gates all background jobs on user downtime and LLM idle state.
- `DaemonJob` typed enum replaces stringly-typed job IDs for `JobQueue::run_now` and `status`.
- Demand-driven memory condensation: `KnowledgeGraph` emits a `tokio::sync::Notify` on dirty; a listener submits `DaemonJob::MemoryCondensation` to the scheduler.
- Configurable `memory.condensation_top_n` (default 500) replaces hard-coded top-20 hash in condensation pipeline.
- `[scheduler]` config section with `debounce_seconds` (default 5) and `cooldown_seconds` (default 60).
- `LlmWorkerPool` tracks in-flight job count via `in_flight_count()`, exposed through `LlmBackend`.

### Changed

- Replaced fixed 30-second interval loop for auto-condensation with event-driven scheduler.
- `POST /memory/refresh` now uses `force_submit` to bypass scheduler gates.
- Nightly optimization callback now submits condensation through the scheduler instead of direct `run_now`.
- `JobQueue::list_jobs()` added for scheduled-job polling.

### Fixed

- `relationship_type_id` no longer uses `?` on `Result` inside `Option`-returning function (Rust 2024 edition compatibility).

## [0.40.7] — 2026-06-10

### Fixed

- Fact extraction now falls back to parsing the assistant's text content as JSON
  when the LLM does not emit a structured tool call. This resolves intermittent
  extraction failures with backends such as Ollama + Gemma that do not support
  `tool_choice`.
- The daemon guard spawns the background server in its own Unix process group,
  preventing Ctrl-C in the terminal from killing the daemon.
- `generate_and_install_service_file` now ensures config and data directories
  exist before writing the systemd unit, preventing NAMESPACE failures when
  `ReadWritePaths` references missing directories.

## [0.40.6] — 2026-06-10

### Fixed

- Addressed remaining CodeRabbit review feedback for PR #125:
  - Fixed CHANGELOG entry for uninstall.sh redirect typo to use literal characters.
  - Aligned test-only init_at() with production init() by ensuring cache directory exists.
  - Replaced silent unwrap_or((0,)) with explicit match on the fact-count query during auto-merge to avoid treating DB errors as zero facts.
  - Documented the auto-merge threshold (fact_count <= 2) in process_extracted_fact.
  - Optimized category validation in insert_facts_batch to query only referenced category IDs instead of the full table.
  - Tidied SQL formatting in get_facts_by_subject_and_predicate.
  - Documented the alias score adjustment (1.1) in entity search queries.
  - Removed unreachable Windows path checks from the Linux-only resolve_executable_path function.
  - Added defensive mimir substring check in uninstall.sh remove_dir before rm -rf.
  - Removed unused serial_test::serial imports in mimir-core tests.

## [0.40.5] — 2026-06-10

### Fixed

- Fixed fact-loss bug where multiple atemporal facts with the same subject and predicate but different objects (e.g. multiple hobbies) would incorrectly supersede each other, leaving only the last-inserted fact. The temporal overlap logic in `insert_fact_in_tx` now respects a `MULTI_VALUED_PREDICATES` allow-list (`hobby`, `likes`, `has_pets`, `has_sibling`, etc.) so that independent values for these predicates coexist instead of overwriting one another.

## [0.40.4] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (additional findings addressed):
  - Fixed typo in `scripts/uninstall.sh` where `error()` redirected with `&&2` instead of `>&2`.
  - Fixed `insert_facts_batch` atomicity by calling `ensure_relationship_type_in_tx` inside the batch transaction instead of autocommitting via `ensure_relationship_type`.
  - Moved `preferred_name` alias registration and auto-merge side effects in `process_extracted_fact` to after the dedup/corroboration check, preventing irreversible mutations on duplicate facts.
  - Aligned `generate_service_file` implementation with its docs and test by removing the unused `cache_dir` parameter and updating callers.

## [0.40.3] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (8 findings addressed):
  - Strengthened `normalize_predicate` to handle `name` → `has_name`, `nickname` → `preferred_name`, `favorite_food`/`color`/`colour` variants, and trimmed leading/trailing whitespace.
  - Expanded `LIST_PREDICATES` to include `has_pets`, `has_child`, `has_parent`, `has_sibling`, and `has_partner`.
  - Removed extra whitespace from the `remember` tool description.
  - `remember` tool output now includes actual error messages instead of just counts.
  - Replaced flaky `tokio::time::sleep(200ms)` in chat integration test with a deterministic polling loop and timeout.
  - `spawn_fact_extraction` now skips empty/whitespace-only messages.
  - Renamed `user_message_clone` to `user_message` in `chat_stream_handler` to clarify ownership.
  - Optimized `seed_identity_facts`: replaced full 1,000-fact scan with targeted predicate-specific queries; both identity inserts are now performed atomically via `KnowledgeGraph::insert_facts_batch`.

### Changed

- Added `relationship_type_id`, `get_facts_by_subject_and_predicate`, and `insert_facts_batch` to `KnowledgeGraph` API.

## [0.40.2] — 2026-06-10

### Fixed

- **Chat fact extraction wired up**: The fact-extraction pipeline (`mimir-knowledge/src/extract.rs`) was fully implemented but never triggered from chat. Both `/chat` and `/chat/stream` endpoints now spawn a background task after persisting the assistant response to extract facts from the user message. This fixes the long-standing issue where Mimir could query the knowledge graph but never write to it from conversation.
- **DRY refactor**: Extracted the duplicated extraction-spawning logic into `spawn_fact_extraction` in `mimir-server/src/routes/chat.rs`.

- **`remember` tool**: Registered `RememberTool` in the tool registry so the LLM can proactively write facts during conversation. The tool accepts structured `RememberOutput` and processes each fact through the same validation, dedup, confidence-assignment, and insertion pipeline used by background extraction.
- **System prompt updated**: The injected memory note now tells the LLM to use the `remember` tool whenever the user shares something worth saving.
- **Extraction prompt enriched**: Added detailed predicate standards (e.g., `studied_at` not `attended`, `hobby` not `hobbies`), explicit list-splitting instructions, and deduplication guidance to the fact extraction system prompt.
- **Predicate normalisation**: Rust-side `normalize_predicate` maps common LLM synonyms to canonical names (e.g., `attended` → `studied_at`, `hobbies` → `hobby`).
- **Comma-separated list splitting**: `split_list_objects` expands single facts with comma-separated values into multiple independent facts for allow-listed predicates (e.g., `hobby: "A, B, C"` → three separate `hobby` facts).

### Changed

- **Documentation**: Updated `docs/fact-extraction-pipeline.md`, `docs/chat-server.md`, and `docs/wiki/fact-extraction.md` to reflect that extraction is now live in the daemon.

## [0.40.1] — 2026-06-10

### Fixed

- **Personality prompts**: Removed references to a non-existent `memory` tool from `transparent`, `concise`, and `warm` presets. The LLM was instructed to use a tool that was not registered, causing `ToolError::NotFound("memory")` during conversations.
- **Identity seeding**: When the server starts, it now inserts `has_name` and `preferred_name` facts into the knowledge graph for the user entity (if not already present). This ensures Mimir can learn the user's identity through the existing memory condensation pipeline instead of relying on prompt injection.

## [0.40.0] — 2026-06-10

### Added

- **Issue #63**: Comprehensive testing suite for `mimir-knowledge`.
  - Inline unit tests for confidence model, clock, entity/fact models, and forget logic.
  - Temporal point-in-time DB integration test (`tests/temporal_point_in_time.rs`).
  - Criterion benchmark suite (`mimir-knowledge/benches/kg_benchmarks.rs`) with 10k-fact dataset covering entity resolution, FTS5, graph traversal, inference chain, and memory condensation.
  - `Clock::today()` and `MockClock::advance(Duration)` for deterministic temporal testing.

### Changed

- `MockClock::advance_seconds(i64)` replaced with `advance(&self, duration: Duration)`.


## [0.39.0] — 2026-06-10

### Added

- **Issue #61**: Full `mimir kb` CLI command suite (Phase A).
  - New commands: `kb query`, `kb show`, `kb edit`, `kb browse`, `kb profile`.
  - Existing commands (`kb audit`, `kb forget`, `kb restore`, `kb trash`) rewritten to go through the daemon via HTTP instead of opening SQLite directly.
  - All commands support `--json` for scripting output.
  - Human-readable output uses `tabled` for tables and `colored` for confidence color-coding (green >0.9, yellow 0.7–0.9, red <0.7).
  - Server routes added under `/kb/`: `query`, `facts/:id`, `facts/forget`, `browse`, `profile`, `audit`, `trash`, `trash/restore`.
  - Shared API types added to `mimir-api-types`: `FactQueryParams`, `FactDetailResponse`, `FactEditRequest`, `BrowseRequest`, `ProfileRequest`, `AuditQueryRequest`, `ForgetRequest`, `RestoreRequest`, `TrashListResponse`, and supporting row types.
  - New `update_fact` method in `mimir-knowledge` for structured field editing with transactional audit logging.
  - Server integration tests for all new routes.

### Changed

- CORS configuration now allows `PATCH` and `DELETE` methods.

## [0.38.0] — 2026-06-09

### Changed

- **Issue #112**: Switched chat context injection wording from `## Persistent Memory Context` to `Key facts I know about you:`.
  - Signals to the LLM that the injected memory is a curated subset, not an exhaustive record.
  - LLM should continue to use KG tools (`kg_query`, `kg_search`) for deeper or exhaustive queries.
- Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs` to use the new wording.
- Updated unit and integration tests in `mimir-core` to assert the new prompt text.

### Added

- Added server integration tests in `mimir-server/src/lib.rs`:
  - `test_chat_injects_kg_memory_into_system_prompt`: verifies blocking `/chat` injects KG condensed memory into the system prompt.
  - `test_chat_stream_injects_kg_memory_into_system_prompt`: verifies SSE `/chat/stream` injects KG condensed memory into the system prompt.


## [0.37.0] — 2026-06-08

### Removed

- **Issue #111**: Deleted the legacy `memory.md` file-backed memory system entirely.
  - Removed `mimir-core/src/memory/` directory (`MemoryManager`, `MemoryLoader`, `MemorySnapshot`).
  - Removed `MemoryTool` from `mimir-core/src/tools/builtins/`.
  - Removed `memory_manager` benchmark from `mimir-core`.
  - Cleaned stale `# path = "${CONFIG_DIR}/memory.md"` example comments from config TOML strings.

### Changed

- Memory is now exclusively knowledge-graph-backed via `mimir-knowledge`.
- `mimir-core` no longer exports a `memory` module; all memory access flows through `mimir-knowledge::KnowledgeGraph`.


## [0.36.0] — 2026-06-08

### Removed

- **Issue #110**: Removed all remaining file-based memory.md scaffolding.
  - memory.path and MIMIR_MEMORY_PATH env override removed from MemoryConfig.
  - MemoryTool unregistered from daemon and CLI tool list.
  - MemoryLoader::init() no longer called during mimir init.
  - AppState no longer carries memory_path or syncs memory.md on shutdown.
  - StatusResponse no longer includes memory_path.
  - mimir-core/src/paths.rs no longer exports memory_path().

### Changed

- mimir memory CLI and /memory server route now exclusively serve knowledge-graph-backed condensed memory.
- mimir status and chat REPL /status display no longer show the deprecated memory.md path.

### Added

- CLI parsing test for mimir memory --refresh flag.

### Documentation

- Updated docs/memory-system.md, docs/cli.md, docs/chat-server.md, docs/shutdown.md, docs/wiki/memory.md, docs/wiki/what-works-now.md, docs/wiki/cli-commands.md, docs/wiki/configuration.md, and docs/wiki/tools.md to remove memory.md references and describe the KG-backed system.

## [0.35.3] - 2026-06-08

### Fixed
- Fixed `sqlx::migrate!` not recognising `-- no-transaction` in migrations 031, 032, and 033 because the directive was preceded by comment headers. This caused those migrations to run inside transactions, which in turn caused `PRAGMA foreign_keys = OFF` to be ignored. Migration 033's `DROP TABLE relationship_types` then triggered an `ON DELETE CASCADE` that silently emptied `relationship_constraints`, breaking `test_predicate_validation`.


## [0.35.2] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback (CodeRabbit AI):
  - Removed duplicate 0.35.1 section from CHANGELOG.
  - Fixed oversize LLM output handling in memory condensation to use deterministic fallback instead of truncation, preventing underflow at `char_limit == 0`.
  - Recurring event output now uses the computed next occurrence date instead of the stored historical date.
  - Search failures during user entity resolution are now handled separately from "not found", preventing duplicate entity creation on transient errors.
  - Memory condensation job failures are now propagated to the job queue result instead of being silently swallowed.
  - Auto-trigger condensation loop is now skipped when no user entity is configured, preventing perpetual 30-second re-triggers.
  - `mimir init` now falls back to system identity when blank/whitespace input is provided.
  - `mimir memory --refresh` now surfaces server-side errors in the CLI output and exits with a non-zero status on failure.
  - Added client tests for `memory_refresh()` success and error paths.
  - Added server route tests for `/memory/refresh` non-loopback rejection, not-registered, and already-running cases.

## [0.35.1] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback:
  - Status endpoint now reads live condensed memory and upcoming section from the knowledge graph instead of the deprecated `memory.md` file.
  - `condensation_dirty` flag now automatically triggers the memory condensation job via a background watcher in the daemon.
  - Removed unused `whoami` dependency from `mimir-core`.
  - Removed dead `condensation_queued` field from `AppState`.
  - Centralised `recurrence_type_id` to `RecurrenceType` mapping via `TryFrom<i16>` in the enums module.
  - Chat system prompt builder now logs warnings when knowledge graph memory queries fail.
  - DRYed the SQL query in `build_memory_schema_with_opts` by constructing it once with a conditional predicate.
  - Fixed budget truncation loop so facts in `exclude_from_budget` buckets are still collected after the character budget is exhausted.

## [0.35.0] — 2026-06-07

### Added
- **Live Memory System (Issue #109)** — Replaced static `memory.md` with an event-driven, knowledge-graph-backed memory block.
  - Stable facts are condensed by the LLM and cached in `system_state.condensed_memory`.
  - Upcoming events (entity dates + temporal facts) are rendered fresh on every request.
  - Regeneration triggers: fact mutations, explicit `mimir memory --refresh`, and nightly optimization completion.
  - Pure formatting LLM prompt with deterministic fallback on failure or oversized output.
  - Sensitive facts are excluded from the LLM condensation pipeline.
- **Identity configuration** — `mimir init` now prompts for full name and preferred name, stored in `[identity]` config section.
- **User entity auto-resolution** — Daemon resolves the user entity from config at startup, creating it in the KG if missing.

### Changed
- `/memory` HTTP route now returns the live condensed memory block instead of `memory.md`.
- Chat system prompt now injects the live memory block from the knowledge graph.
- `build_memory_schema` supports `exclude_buckets` and `exclude_sensitive` options.
- `OptimizationRunner` now supports an `on_complete` callback for post-optimization hooks.

### Deprecated
- `memory.md` file-based memory is deprecated. `MemoryTool` writes are now logged as warnings.

## [0.33.2] - 2026-06-05

## [0.34.2] - 2026-06-07

### Fixed

- **Addressed PR #113 review feedback** (CodeRabbit AI review round 2):
  - Added serde default for `memory_priority_id` in `Fact` model to preserve legacy trash payload deserialization.
  - Replaced magic priority ID fallback (`3`) with semantic SQL lookup against `memory_priorities` table.
  - Fixed fire-and-forget centrality cache updates by making `bump_centrality` and `drop_centrality` async.
  - Eliminated TOCTOU race in `build_memory_schema` cache population with a read-then-populate pattern.
  - Replaced hardcoded category ID lists in `determine_bucket` with named constants.
  - Fixed potential UTF-8 panic in `truncate_fact` with char-aware truncation.
  - Reformatted SQL strings across `trash.rs` and `inference_tests.rs` for readability.
  - Updated documentation version references and corrected incomplete sentences.

## [0.34.1] - 2026-06-06

### Fixed

- **Review fixes for PR #108**: addressed 3 critical review findings in fact ranking engine.
  - Wired up `memory_priority_id` from `relationship_types.default_memory_priority_id` during fact insertion (`queries/fact.rs`, `extract.rs`, `models/fact.rs`).
  - Moved `drop_centrality` cache decrements to occur **after** `forget_fact` database transaction succeeds (`lib.rs`), preventing permanent cache drift on DB errors.
  - Fixed `truncate_fact` budget edge case (`queries/memory.rs`) so that when remaining budget is smaller than `subject + relationship + 3` overhead, `object_display` is correctly truncated to `…` instead of silently exceeding the budget.


### Fixed

- **Review fixes for PR #107**: addressed 10 CodeRabbit review findings across knowledge graph, server, and CLI.
  - `extract.rs` prompt now includes sub-categories with indentation so the LLM can pick specific IDs.
  - `lib.rs` fact insertion now validates category IDs before `INSERT OR IGNORE`, failing loudly on non-existent categories.
  - `queries/category.rs` replaced magic `NOT IN (5, 6)` with bound `FactStatus::Superseded` / `Forgotten` parameters.
  - `kg_expand_catalogue.rs` now queries real `fact_count` for each child category instead of hard-coding `0`.
  - `integration_tests.rs` merge assertion tightened with `object_id` filter to avoid false positives.
  - `error.rs` no longer leaks raw internal KG error strings in `500` HTTP responses.
  - `lib.rs` (server) tool-registry tests now assert `expand_catalogue` and `get_facts_in_catalogue` are exported.
  - `chat.rs` only fetches the catalogue DB when a new session or incognito turn starts, avoiding hot-path latency.
  - `cli.rs` `category add` now exposes `--memory-weight` to match the server API.
  - `kb.rs` JSON decode failures are no longer swallowed with `unwrap_or_default()`; they now surface as fatal CLI errors.

## [0.33.1] - 2026-06-05

### Fixed

- **P2**: `get_facts_matching_all_categories` now deduplicates input category IDs before querying, preventing empty results when duplicate IDs are passed.
- **P3**: Removed unused `client` variable in `mimir/src/kb.rs` (`handle_kb_category`).
- **P3**: Simplified redundant closures in `mimir-server/src/routes/kb_categories.rs` (5 instances of `.map_err(|e| error::knowledge_error(e))?` → `.map_err(error::knowledge_error)?`).

## [0.32.2] - 2026-06-05

### Fixed

- **Review fixes for PR #92**: addressed 14 CodeRabbit review findings across job queue, optimization pipeline, documentation, and daemon routes.

## [0.32.1] - 2026-06-04

### Fixed

- **P1**: `optimization_pass_runs` now linked to parent `optimization_runs` via foreign key `run_id`. `OptimizationRunner` inserts a parent row at pipeline start and updates it on completion or failure. Failed passes are recorded with error text instead of being silently omitted.
- **P1**: `DailySchedule::next_after` now converts the stored naive local time to UTC using `chrono::Local`, fixing scheduling for non-UTC timezones.
- **P1**: `chat_stream_handler` now calls `state.record_user_activity()`, ensuring SSE stream interactions update `last_user_activity` and prevent premature job yielding.
- **P2**: `JobQueue::run_now` now rejects concurrent executions of the same job by checking for an existing `Running` row in `job_runs`.
- **P2**: `semantic_dedup` candidate query now includes `ORDER BY a.id, b.id` for deterministic candidate selection.
- **P2**: `semantic_dedup` now uses a structured LLM tool schema (`evaluate_dedup_candidates`) instead of relying on raw JSON parsing from a plain-text prompt.


## [0.32.0] - 2026-06-04

### Added

- JobQueue and nightly optimization pipeline (issue #58):
  - New `mimir-core::job_queue` with durable job definitions, runs, scheduling, and manual triggers.
  - `JobQueue` persisted in `jobs.db` with `Job`, `JobPriority`, `JobStatus`, `JobRunStatus`, `DailySchedule`, `JobContext`, and `JobRunSummary` public types.
  - Config support for `[knowledge.optimization]` defaults: `cpu_cores = 1`, `nice_level = 10`, `timeout_minutes = 120`, `schedule_time = "02:00"`.
  - Daemon tracks user activity in `AppState`; chat routes record interaction time.
  - System jobs yield between pass boundaries when user activity is inside the 5-minute idle window.
  - Daemon routes: `GET /kb/optimization/status` and `POST /kb/optimization/run-now` (loopback-only for run-now).
  - CLI commands: `mimir kb optimization --status` and `mimir kb optimization --run-now`.
  - Refactored `mimir-knowledge/src/optimization` into pass modules with 10 nightly passes (7 core optimization passes plus 3 cleanup steps):
    - Pass 1: deterministic dedup (exact triple merge).
    - Pass 1b: semantic dedup via LLM structured JSON; auto-merge >= 0.9 confidence, queue uncertain pairs.
    - Pass 2: contradiction resolution.
    - Pass 3: inference chain re-evaluation.
    - Pass 4: confidence recalculation.
    - Pass 5: dormant cleanup (old disputed non-user facts).
    - Pass 6: pattern consolidation stub.
    - Pass 7: compaction (FTS rebuild, ANALYZE, VACUUM).
    - Plus: pending confirmation cleanup (7-day TTL) and trash cleanup.
  - Pre-pass backup with `VACUUM INTO` to `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db` with counter suffix for collisions.
  - Per-pass run recording in `optimization_pass_runs` table.
  - Integration tests for daemon routes and CLI client methods.

### Changed

- `run_nightly_optimization` compatibility wrapper now delegates to `OptimizationRunner::run_all`.
- `cascade_inner` in `confidence.rs` future is now `Send`-safe.

## [0.31.1] - 2026-06-04

### Fixed

- **P1**: `restore_all` now maps both child and parent IDs through `id_map` when rebuilding `fact_dependencies`, preventing FK violations on restored facts.
- **P1**: `restore_fact` now marks the trash row as restored, preventing duplicate restores and stale trash listings.
- **P1**: `hard_delete_all_facts` correctly reports the number of forgotten facts via `rows_affected()` instead of querying the now-empty table.
- **P1**: `create_backup` escapes single quotes in the backup path before interpolating into `VACUUM INTO`, preventing SQL injection/breakage from `XDG_DATA_HOME` paths containing apostrophes.
- **P2**: Restoration audit log now references the newly generated fact ID instead of the original deleted ID.

## [0.31.0] - 2026-06-04

### Added

- Phase 2: Forgetting system -- trash, cascade forget, restore, bulk operations (#57)
  - Bulk forget by predicate, entity, source, time range, and full reset.
  - Trash bin with 30-day expiry, restoration, and automatic nightly cleanup.
  - Cascade forget for inferred facts: orphan removal and confidence recalculation.
  - Bulk safeguards: >100 facts requires --yes, sensitive predicates require --confirm-sensitive, full reset requires typing DELETE EVERYTHING.
  - Full reset creates a timestamped SQLite backup via VACUUM INTO.
  - New CLI commands: mimir kb forget, mimir kb restore, mimir kb trash.
  - Extended TrashPayload with dependency chains so restored facts rebuild parent links.
  - Sensitive predicate flag (sensitive BOOLEAN) on predicates table with seeded defaults for medical/financial terms.


## [0.30.1] - 2026-06-04

### Fixed

- **P1**: `kg_query` and `kg_related` no longer mutate the database via `ensure_predicate` during read-only tool calls. Both now use the new read-only `get_predicate_id` lookup; missing predicates return empty results instead of silently inserting rows.
- **P2**: `AppState` knowledge graph and context database fallbacks now propagate `PathsError` instead of using a broken tilde (`~`) literal path.
- **P3**: `kg_search` now returns an explicit invalid-arguments error when an unrecognized `entity_type` is supplied, rather than silently ignoring the filter.

## [0.30.0] - 2026-06-04

### Added

- Phase 2: Knowledge Graph LLM tools — `kg_query`, `kg_related`, `kg_search` (#56)
  - Database migration `028_add_performance_indexes.sql` for tool query performance.
  - Query layer: `search_entities`, `traverse_graph`, `get_facts_by_subject_filtered`, `get_entity_names`.
  - Tool implementations in `mimir-knowledge/src/tools/` implementing `mimir_core::Tool`.
  - Server integration: `AppState` initialises `KnowledgeGraph` and registers all three tools.
  - Input sanitisation, FTS5 injection defence, and SQL-level exclusion of pending/superseded/forgotten facts.
  - Comprehensive unit and integration tests.


## [0.29.2] - 2026-06-03

### Fixed

- `mimir-knowledge/src/optimization/mod.rs`: `cleanup_stale_pending_confirmations` now deletes `fact_dependencies` rows before deleting the fact and wraps each deletion in a transaction, avoiding `ON DELETE RESTRICT` violations and ensuring atomic DB/cache state.

## [0.29.1] - 2026-06-03

### Fixed

- `mimir-knowledge/src/extract.rs`:
  - `confirm_fact` now cascades inferred facts instead of discarding them (P1).
  - `find_existing_fact` dedup query now matches pending-confirmation facts, preventing duplicate sensitive extractions (P1).
  - `handle_correction` retrospective loop is now atomic: all overlapping facts are marked `Corrected` and soft-deleted in a single transaction before child evaluation (P2).
- `mimir-knowledge/tests/extraction_test.rs`: corrected misleading comment in `test_casual_extraction` (P3).

## [0.29.0] - 2026-06-03

### Added

- Fact extraction pipeline (issue #55):
  - `mimir-knowledge/src/extract.rs`: full LLM → Rust validation → entity resolution → confidence assignment → sensitive confirmation → fact insertion pipeline.
  - LLM tool `remember`: structured schema for extracting subject-predicate-object triples with classification (Explicit / Casual / Correction), temporal bounds, and sensitivity flags.
  - Entity resolution: names matched via exact → alias → FTS5 fuzzy; new entities auto-created with LLM-provided type.
  - Confidence assignment: classification maps to `SourceType` → `confidence::initial()`; LLM hints are ignored.
  - Correction handling:
    - Temporal: `correction_scope` as ISO-8601 datetime closes the sole open-ended predecessor.
    - Retrospective: `correction_scope = "always"` marks overlapping facts as `Corrected`, moves them to trash, and inserts the new fact.
  - Sensitive fact confirmation flow:
    - Sensitive facts inserted as `Disputed` with `pending_confirmation = TRUE`.
    - In-memory `HashSet<i32>` cache rebuilt from DB on startup.
    - `confirm_fact`: flips to `Active`, confidence `1.0`, triggers inference.
    - `reject_fact`: hard-deletes with `Rejected` audit entry.
  - Corroboration stub for issue #79: duplicate facts returned in `ExtractionOutcome::corroborated` without insertion.
  - 11 integration tests covering explicit, casual, entity resolution, temporal/retrospective correction, sensitive confirmation/rejection, multiple facts, empty extraction, and invalid LLM output.

### Changed

- `facts` table: added `pending_confirmation BOOLEAN NOT NULL DEFAULT FALSE` (migration 026).
- `change_types` table: added `rejected` (migration 027).
- `Fact` model: added `pending_confirmation` field.
- `ChangeType` enum: added `Rejected = 8`.
- `ranges_overlap` in `queries/fact.rs`: made `pub` for reuse in extraction pipeline.


## [0.28.1] - 2026-06-02

### Fixed

- Review feedback on inference engine (issue #54):
  - `CHANGELOG.md`: reordered 0.28.0 section to top with markdownlint blank lines.
  - `docs/inference-engine.md`: explicit facts are detected by `!inferred` rather than `confidence == 1.0`.
  - `mimir-knowledge/src/inference/mod.rs`: streaming evaluation for `evaluate_batch` (pending — rule loop still materialises; moved to follow-up).
  - `contradiction.rs`: explicitness uses `!inferred`; status updates wrapped in atomic transactions via `set_status_tx`.
  - `threshold.rs`: DB errors propagated instead of `unwrap_or(0)`; stale preferences deleted when source fact missing; duplicate `StatusChange` audit entries deduplicated within 24h.
  - `transitivity.rs`: trigger queries include `FactStatus::Inferred`; inferred facts use temporal intersection of parent windows.
  - `lib.rs`: `ensure_predicate` insert is atomic with `ON CONFLICT`.
  - `NewFact`: removed `Default` impl; added `NewFact::new(subject_id, predicate)` constructor.
  - `optimization/mod.rs`: confidence cascade uses unlimited depth (`None`); operational errors propagated instead of swallowed.
  - Tests: predicate name roundtrip restored; unknown predicate test uses absent ID; contradiction relation type asserted; cycle-safety contract replaces brittle exact count.

## [0.28.0] - 2026-06-02

### Added

- Inference engine core with `InferenceRule` trait, `RuleEngine`, and `CascadeContext` for cycle-safe unbounded cascades.
- Transitivity rule: `visited`/`is_in` + `is_in` chain → inferred transitive facts with depth-tracked confidence.
- Contradiction rule: real-time `Disputed` status + bidirectional `Contradicts` edges; nightly batch auto-resolves explicit > inferred disputes.
- Threshold rule: 3+ `rejected_action` facts → `General` preference upsert; nightly re-count warns if threshold drops.
- `PredicateRegistry` with `ensure_predicate` and `predicate_name` for unlimited extensible predicates backed by the DB.
- Migrations 024 (Contradicts relation type) and 025 (rejected_action predicate).
- Nightly optimization orchestrator (`run_nightly_optimization`) wiring contradiction resolution, confidence propagation, and inference re-evaluation.
- Integration tests for transitivity, contradiction, threshold, cascade, and cycle safety.

### Changed

- Removed compile-time `Predicate` enum; `NewFact.predicate` is now a `String` resolved at runtime.
- `Fact::predicate()` removed; callers use `kg.predicate_name(fact.predicate_id)`.
- `KnowledgeGraph::insert_fact` automatically runs inference rules and cascades inferred facts.
- `NewFact` extended with `inferred`, `inference_depth`, `confidence`, and `parent_fact_ids` fields.

### Documentation

- Added `docs/inference-engine.md` with architecture, rule descriptions, confidence formulas, and cascade behavior.
- Added `docs/wiki/inference-rules.md` with user-facing examples and best practices.

## 0.27.1 (2026-06-02)

> Next-day hotfix release for 0.27.0.

### Fixed

- Atomic upsert: delete and insert now happen in a single transaction, preventing data loss on crash between commit and insert.
- Contextual lookup now correctly falls back to the default (zero-context) preference when no contexts match, instead of ranking by confidence.
- `preference_sources` now binds `extracted_at` explicitly for deterministic timestamps.
- `preference_audit_log` stores `NULL` for the `reason` column on creation events instead of an empty string.
- `get_preference` eliminates N+1 queries by fetching all contexts in a single query.
- Uniqueness checks in `insert_preference` and `upsert_preference` no longer clone the full context `HashSet`.
- Confidence validation now happens before acquiring a database write lock.
- Migration 023 now seeds `predicate_constraints` for `HasPreference` so `validate_predicate` does not fail.

## 0.27.2 (2026-06-02)

### Fixed

- Review feedback on preference system (issue #53):
  - `source_fact_id` is now nullable in `preferences` table and Rust types (`Option<i32>`).
  - Explicit preferences (`overridden_by_user = true`) now require `confidence = 1.0` at validation time.
  - `UpsertAction::Overwritten` now updates the existing preference row in-place instead of deleting and re-inserting, preserving the audit trail.
  - Clarified that the 11 seeded predicates in `predicate_constraints` are the complete set.

## 0.27.0 (2026-06-01)

### Added

- Preference system refactor (issue #53): behavioural index over the fact graph with contextual lookup and conflict resolution.
- New `Predicate::HasPreference = 11` seeded in `predicates` table.
- New lookup tables (re-seeded in migration 023):
  - `preference_categories`: 7 variants — CalendarBehavior, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, General.
  - `preference_source_types`: 3 variants — Interaction, Fact, UserEdit.
- New `PreferenceCategory` and `PreferenceSourceType` enums with `#[repr(i16)]` and `sqlx::Type`.
- New schema (migration 023):
  - `preferences` with `source_fact_id NOT NULL REFERENCES facts(id)`.
  - `preference_contexts` — normalized context conditions, no JSON.
  - `preference_sources` — provenance with `(preference_id, source_type_id, source_id)` unique constraint.
  - `preference_audit_log` — immutable history without FK to `preferences` (preserves history after deletion).
- Contextual lookup API: `get_preference(entity_id, key, query_context)` ranks by match count, confidence, and recency.
- Upsert API with conflict resolution:
  - Explicit overrides inferred.
  - Higher-confidence inferred wins.
  - Same confidence keeps existing.
  - `overridden_by_user = true` blocks inferred overwrites.
- Full audit logging on preference creation and overwrite.
- Source tracking for every preference.
- FK enforcement: non-existent `source_fact_id` is rejected.
- Comprehensive test suite in `mimir-knowledge/tests/preference_tests.rs` (15 tests).
- Technical documentation: `docs/preference-system.md`.
- User-facing documentation: `docs/wiki/preferences.md`.

### Changed

- **Breaking schema change:** old `preferences` and `preference_sources` tables dropped and recreated. No data migration attempted.

## 0.26.0 (2026-06-01)

### Added

- New built-in tool `get_weather` using wttr.in.
  - Fetches current conditions for any location (city name, airport code, or coordinates).
  - Returns structured JSON: temperature (°C/°F), feels-like, description, humidity, wind, UV index, visibility, and pressure.
  - Configurable base URL for testing (`GetWeatherTool::with_base_url`).

## 0.25.1 (2026-06-01)

### Fixed

- `get_active_facts_at` restored missing `AND fact_status_id = ?` filter so it again returns only active facts.
- `query_audit_log` switched from INNER JOINs to LEFT JOINs on `facts`, `entities`, and `predicates`, ensuring audit history remains visible after a fact is forgotten (hard-deleted).
- `mimir kb audit` now validates `--from` and `--to` datetime strings and exits with an error instead of silently ignoring malformed input.

## 0.25.0 (2026-06-01)

### Added

- Provenance audit refactor (issue #52): typed `change_type` and `changed_by` lookup tables with integer IDs.
- New lookup tables: `extraction_methods` (5 variants), `change_types` (7 variants), `changed_by_types` (4 variants).
- New `ExtractionMethod`, `ChangeType`, and `ChangedBy` enums with `#[repr(i16)]` and `sqlx::Type`.
- `mimir kb audit` CLI command for querying the fact audit log directly from the local SQLite database.
- `query_audit_log` API with filters: entity name, predicate name, datetime range, and change type.
- `add_source_to_fact` API for adding corroborating sources to an existing fact.
- `sources` unique constraint: `(fact_id, source_type_id, connector_id, raw_reference)`.
- Audit entries are now column-only JSON snapshots (e.g. `{"valid_until": ...}`) instead of full fact snapshots.

### Changed

- **Breaking schema change:** `source_types` remapped to 6 canonical variants: `UserEdit(1)`, `Connector(2)`, `Inference(3)`, `Interaction(4)`, `Import(5)`, `System(6)`. Old `Email`/`Calendar`/`Photo`/`Message` variants mapped to `Connector`; `CasualMention` mapped to `Interaction`.
- `fact_audit_log` recreated with `change_type_id`, `changed_by_id`, `reason`, and `changed_at` columns. Old action/performer strings migrated via best-effort mapping.
- `sources` recreated with `extraction_method_id INTEGER REFERENCES extraction_methods(id)`.
- `NewFact` expanded with `connector_id`, `connector_type`, `raw_reference`, and `extraction_method` fields.
- `update_fact_valid_until`, `update_fact_status`, and `forget_fact` now accept `ChangedBy` parameter.
- `forget.rs` deletes **all** `fact_dependencies` rows where the forgotten fact is parent or child (not just `InferredFrom`).
- Confidence cascade now writes `confidence_change` audit entries on child recalculation.

### Fixed

- Prevent duplicate edges when an already-superseded fact is superseded again by a third explicit fact.
- Correct `children` and `remaining_parents` queries in `forget.rs` after removal of relation_type filter from the DELETE query.

## 0.24.3 (2026-05-31)

### Added

- Structural confidence model (issue #51): confidence derived entirely from graph structure, zero LLM involvement, zero time-based decay.
- New `SourceType` variants: `CasualMention`, `Import`, `System`.
- New `ConnectorType` enum with SQLite lookup table and reliability tracking.
- `inference_confidence` formula: signed parent sum × chain penalty (0.8^depth) × breadth factor.
- `inference_depth` and `stale_confidence` columns on `facts` table.
- `is_positive` column on `fact_dependencies` for signed parent contributions.
- Per-connector reliability scores with feedback loop (`adjust_connector_reliability`).
- Eager bounded confidence cascade on parent removal.

### Changed

- `NewFact` no longer accepts caller-provided `confidence`; confidence is now computed in Rust (internal change; not public API).
- Connector-type source facts now use per-connector reliability scores instead of flat 0.80.
- Initial confidence values: `UserEdit`/`System` = 1.0, `CasualMention` = 0.30, `Import` = 0.80.

### Fixed

- Updated all test assertions and raw SQL to match new schema columns.

## 0.24.4 (2026-05-31)

### Fixed

- Build failure in `mimir-client`: replaced unsupported `reqwest` feature `rustls-tls-ring` with `rustls-native-certs` to align with `reqwest` 0.13 feature flags and `mimir-core` crate configuration.

### Documentation

- Added `docs/wiki/what-works-now.md`: comprehensive user-facing overview of all working features, current limitations, known bugs, and roadmap context.

## [0.33.0] - 2026-06-05

### Added

- **Category taxonomy system** (Dewey Decimal-style):
  - New `categories` table with hierarchical parent-child relationships.
  - `fact_categories` junction table allowing facts to belong to multiple categories.
  - Comprehensive seed taxonomy covering Identity (100), Food & Drink (200), Health (300), Relationships (400), Work (500), Home (600), Entertainment (700), Travel (800), and Schedule (900) with 2-3 levels of depth.
  - New KG tools: `expand_catalogue` and `get_facts_in_catalogue` for LLM-driven category browsing and fact retrieval.
  - System prompt injection of top-level catalogue so the LLM knows what knowledge domains exist.
  - CLI commands: `mimir kb category list`, `show`, `add`, `delete`.
  - Server routes: `GET /kb/categories`, `GET /kb/categories/{id}`, `POST /kb/categories`, `DELETE /kb/categories/{id}`.

- **Extraction pipeline category assignment**:
  - LLM suggests 1–3 category IDs per extracted fact via the `remember` tool.
  - Rust validates all suggested IDs against the database before insertion.

### Changed

- **Renamed `predicates` → `relationship_types`** and `predicate_constraints` → `relationship_constraints` across the entire codebase (DB schema, models, queries, tools, inference rules, tests).
- Updated all SQL queries, indexes, and foreign keys to use `relationship_type_id`.
- Updated `MemoryManager` and system prompt integration to read from the knowledge graph catalogue.

### Migration

- Migration `031_category_taxonomy_and_rename_predicates.sql` performs the rename and seeds the full category taxonomy.

## [0.34.0] - 2026-06-06

### Added

- **Issue #108**: Fact Ranking & Selection Engine (`mimir-knowledge`).
  - Introduced `memory_priorities` lookup table (Critical, High, Normal, Low) and `memory_priority_id` on `facts`.
  - Added `default_memory_priority_id` to `relationship_types` for automatic priority assignment at insertion.
  - Implemented scoring formula: `confidence × category.memory_weight × temporal_boost × priority_boost × centrality_boost`.
  - Temporal boost: `10.0 / sqrt(max(days, 0.5))` for future-dated facts (upcoming events, birthdays).
  - Centrality boost: entity connection count with in-memory `HashMap` cache, incrementally updated on mutation.
  - Budget fill algorithm: identity facts first (~200-char soft reservation), then greedy score-based fill to 2500-char limit.
  - Structured buckets: `identity`, `relationships`, `preferences`, `upcoming`, `general`.
  - Deterministic fallback renderer in Rust for when LLM condensation is unavailable.
  - `system_state` read/write queries for cached `condensed_memory`.
  - Unit and integration tests covering scoring, temporal boost, budget fill, renderer, and centrality cache.

## [0.38.1] — 2026-06-09

### Added

- **Issue #60**: Added explicit non-exhaustive note to context-injected system prompt.
  - When condensed memory is present, the system prompt now appends: "Note: This is not an exhaustive list. Use kg_query, kg_related, or kg_search tools if you need more information."
  - Signals the LLM that the injected memory is a curated subset, prompting tool use for deeper queries.
  - Completes the Layer 2 context injection design from Phase 2 Knowledge Graph architecture.
  - Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs`.
  - Updated unit tests, integration tests, and server integration tests to assert the note is present.
  - Updated documentation in `docs/personality-system.md`, `docs/wiki/personality.md`, and `docs/wiki/what-works-now.md`.
