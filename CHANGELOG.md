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
  - Refactored `mimir-knowledge/src/optimization` into pass modules with 7-pass nightly pipeline:
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

# Changelog

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
