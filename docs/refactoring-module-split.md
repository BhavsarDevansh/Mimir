# Module-Split Refactor (0.94.0)

## Rationale

Several files had grown far beyond a single responsibility, with the worst exceeding four thousand lines. A file that large makes navigation, review, and reasoning about invariants hard: related logic is scattered across thousands of lines, private helpers are shared implicitly, and the compiler must re-check one giant module for every small change. This refactor breaks each oversized file into a directory of small, single-concern modules while preserving the exact public API surface of every crate.

Splitting was done case by case, not mechanically. Files that were large but cohesive (a single struct with one responsibility, e.g. `mimir-core/src/scheduler.rs` or `mimir-api-types/src/kb.rs`) were kept intact; files that mixed several concerns (e.g. the calendar connector mixing construction, credentials, sync, payloads, and trait plumbing) were separated by concern.

## Patterns used

- **Directory-per-concern:** each split file became `<name>/mod.rs` plus sibling modules, e.g. `mimir-connectors/src/supervisor.rs` → `mimir-connectors/src/supervisor/{mod,config,error,trigger,runner,control,cycle}.rs`.
- **Re-export from the root module:** `mod.rs` re-exports the public items (`pub use runner::ConnectorSupervisor;`) so every existing `use` path keeps working unchanged.
- **`pub(super)` visibility:** cross-module helpers within a split directory use `pub(super)` instead of `pub(crate)` so the surface stays minimal and the module graph is explicit.
- **Test modules follow their subject:** unit tests moved into the module they test (e.g. slug-validation tests live in `secrets/store.rs`); large integration suites were split into per-concern files (e.g. `tests/calendar_connector.rs` → `calendar_sync_tests.rs`, `calendar_factory_tests.rs`, `calendar_extract_tests.rs`, `calendar_writeback_tests.rs`, `calendar_kb_tests.rs`) sharing fixtures via `tests/common/mod.rs`.
- **No behaviour changes:** the refactor is purely structural. The full workspace test suite (1476 tests), `cargo fmt`, and `cargo clippy --workspace --all-targets` (zero warnings) were green before and after.

## Module maps after the split

### mimir-connectors (library)

| Directory | Concern |
|-----------|---------|
| `src/supervisor/` | supervised connector lifecycle: `config` (tunables), `error`, `trigger` (manual-sync types), `runner` (struct + spawning), `control` (start/stop/pause/trigger/action dispatch), `cycle` (per-connector runner loop) |
| `src/calendar/` | calendar backend: `construct`, `credentials`, `sync`, `trait_impl`, `payload`, plus `caldav/{client,ical,xml}` transport |
| `src/email/` | email backend: `config`, `factory`, `imap`, `connector/{construct,credentials,extract,session,trait_impl}`, `jsonld/{facts,html,nodes,reservations,values}`, `llm/{message,parse,schema}` |
| `src/rate_limit/` | rate limiting: `config`, `error`, `limiter`, `quota`, `retry` |
| `src/geocoder/` | geocoding: `client`, `parse` |
| `src/ical/` | iCal parsing: `parse`, `facts` |
| `src/mock/` | mock connector harness: `config`, `connector`, `factory`, `recorder`, `sync_impl` |
| `src/photos/` | photos backend: `config`, `connector`, `cursor`, `exif`, `factory`, `scan`, `sync`, `watcher` |
| `src/secrets/` | credential storage: `error`, `bundle`, `store` (trait + slug validation), `file` (V1 default), `memory` (test/helper) |

### mimir-connectors (integration tests)

| Before | After |
|--------|-------|
| `tests/calendar_connector.rs` (1141 lines) | `tests/calendar_sync_tests.rs`, `tests/calendar_factory_tests.rs`, `tests/calendar_extract_tests.rs`, `tests/calendar_writeback_tests.rs`, `tests/calendar_kb_tests.rs` |
| `tests/supervisor_lifecycle.rs` (922 lines) | `tests/supervisor_lifecycle_tests.rs`, `tests/supervisor_trigger_tests.rs`, `tests/supervisor_stop_tests.rs` |
| fixtures | shared in `tests/common/mod.rs` |

### mimir-core (library)

| Before | After |
|--------|-------|
| `src/config.rs` | `src/config/{mod,base_url,env,init,load,reload,types,tests}.rs` |
| `src/context.rs` | `src/context/{mod,core,messages,path,schema,search,sessions,trim,tests}.rs` |
| `src/job_queue.rs` | `src/job_queue/{mod,queue,tests}.rs` |
| `src/llm/client.rs` | `src/llm/client/{mod,backend,chat,construct,transport,tests}.rs` |
| `src/llm/pool.rs` | `src/llm/pool/{mod,queue,worker,tests}.rs` |

### mimir-knowledge (library)

| Before | After |
|--------|-------|
| `src/extract.rs` | `src/extract/` (conversational extraction pipeline) |
| `src/normalize.rs` | `src/normalize/` (shared `normalize_and_insert` boundary) |
| `src/forget.rs` | `src/forget/` (fact forgetting/cascade) |
| `src/queries/entity.rs` | `src/queries/entity/{mod,crud,dedup,locations,names,nearby,predicates}.rs` |
| `src/queries/fact.rs` | `src/queries/fact/{mod,browse,conflict,corroboration,insert,pending,read,status,update}.rs` |
| `src/queries/memory.rs` | `src/queries/memory/{mod,build,ranking,render,tests}.rs` |
| `src/queries/preference.rs` | `src/queries/preference/{mod,read,write}.rs` |
| `src/optimization/mod.rs` (890 lines) | `src/optimization/{mod,runbook,passes,backup,nightly}.rs` |

### mimir-knowledge (integration tests)

| Before | After |
|--------|-------|
| `tests/fact_management_test.rs` (2627 lines) | `fact_crud_test.rs`, `fact_temporal_test.rs`, `fact_audit_test.rs`, `fact_cascade_test.rs`, `fact_confidence_test.rs`, `fact_supersession_test.rs`, `fact_corroboration_test.rs`, `fact_corroboration_cascade_test.rs` |
| `tests/integration_tests.rs` (1333 lines) | `integration_entity_test.rs`, `integration_events_test.rs`, `integration_dedup_test.rs`, `integration_entity_locations_test.rs` |
| `tests/extraction_test.rs` (1333 lines) | `extraction_tool_test.rs`, `extraction_text_fallback_test.rs`, `extraction_rust_overrides_test.rs` |

### mimir-server (library)

| Before | After |
|--------|-------|
| `src/routes/kb.rs` | `src/routes/kb/{mod,browse,detail,forget,helpers,optimization,params,pending,query,trash}.rs` |
| `src/state.rs` | `src/state/{mod,builder,identity,tests}.rs` |
| `src/app.rs` / `src/server.rs` / `src/shutdown.rs` | extracted from `src/lib.rs` into dedicated modules |

### mimir-server (integration tests)

| Before | After |
|--------|-------|
| `tests/kb_tests.rs` (1034 lines) | `kb_query_tests.rs`, `kb_identity_tests.rs`, `kb_pending_tests.rs` |
| `tests/chat_tests.rs` (1012 lines) | `chat_basic_tests.rs`, `chat_tools_tests.rs`, `chat_memory_tests.rs`, `chat_learning_tests.rs` |

### mimir-client

| Before | After |
|--------|-------|
| `src/kb.rs` (695 lines) | `src/kb/{mod,optimization,query,lifecycle,categories,tests}.rs` |

### mimir-api-types

| Before | After |
|--------|-------|
| `src/lib.rs` (wire types in one file) | split into `chat.rs`, `connectors.rs`, `kb.rs`, `kb_maintenance.rs` with `lib.rs` re-exporting |

## Verification

- `cargo fmt --all` — clean.
- `cargo clippy --workspace --all-targets` — zero warnings (the only remaining note is a third-party `proc-macro-error2` future-incompat warning, informational).
- `cargo test --workspace` — 1476 passed, 0 failed (unit, integration, and doc-tests).

## Related documentation

- `docs/wiki/module-split.md` — user-facing summary.
- `docs/workspace.md` — crate-level structure.
- Per-subsystem docs (`docs/connectors-framework.md`, `docs/llm-client.md`, `docs/memory-system.md`, etc.) list the new module locations.
