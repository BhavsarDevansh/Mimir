# Benchmarks

## Overview

Criterion `0.8.2` with the `async_tokio` feature is used for all benchmarks. Benchmarks are located in `mimir-core/benches/` and compiled as standalone binaries (`harness = false`).

## Design Principles

- **Stateful ops use `iter_batched`**: Database and file-backed benchmarks create fresh state per iteration to avoid cross-run contamination.
- **`std::hint::black_box`**: Prevents the compiler from optimising away pure computations (e.g. `usage_pct()`).
- **Synchronous runner for nested async**: `b.iter_batched` with an explicit `rt.block_on(...)` avoids "Cannot start a runtime from within a runtime" panics that occur when `rt.block_on` is called inside a `b.to_async(&rt)` closure.

## Suites

### `context_manager`

| Benchmark | What it measures |
|-----------|------------------|
| `context_create_session` | SQLite `INSERT` + `CREATE TABLE` overhead |
| `context_add_messages` | Two `INSERT` statements (user + assistant) |
| `context_export_messages` | `SELECT * FROM messages` for a 40-message session |
| `context_trim_to_budget` | Token-aware trimming of 50 message pairs |

### `tool_registry`

| Benchmark | What it measures |
|-----------|------------------|
| `tool_registry_register` | Registration of two built-in tools |
| `tool_registry_export_schema` | OpenAI function-calling JSON generation |
| `tool_registry_execute_echo` | `echo` tool execution with JSON args |

### `personality`

| Benchmark | What it measures |
|-----------|------------------|
| `personality_system_prompt_empty` | String concat with empty memory |
| `personality_system_prompt_large_memory` | String concat with 10 000 chars |

## Running Benchmarks

```bash
# All suites
cargo bench -p mimir-core

# Single suite
cargo bench -p mimir-core --bench context_manager
```

## `mimir-knowledge`

| Benchmark | What it measures |
|-----------|------------------|
| `entity_resolution_exact` | Exact name lookup by `get_by_name` with 10k facts |
| `entity_resolution_alias` | Alias lookup with 10k facts |
| `fts5_search` | FTS5 full-text search over entities with 10k facts |
| `facts_by_subject_with_chain` | Retrieve facts by subject with a 3-hop chain present |
| `inference_chain_100` | Transitivity-rule evaluation across a 100-fact `located_in` chain |
| `memory_condensation` | Build and render ranked memory schema from 10k facts |

```bash
# Run all knowledge graph benchmarks
cargo bench -p mimir-knowledge

# Run a single benchmark
cargo bench -p mimir-knowledge --bench kg_benchmarks -- entity_resolution_exact
```

## Pure-helper suites (non-hotpath coverage)

Three suites were added on the `tests-and-benchmarks` branch to benchmark deterministic pure helpers that are easy to skip when focusing only on the hotpath. All use `std::hint::black_box` to defeat optimisation.

### `mimir-api-types` — `wire_types`

| Benchmark | What it measures |
|-----------|------------------|
| `tool_call_info_truncate_{short,long,multiline,emoji}` | `ToolCallInfo::truncate_result` across input shapes (incl. multibyte) |
| `serde_{chat_request,chat_response,status_response,fact_detail,forget_request,audit_query,browse_request,fact_query_params,pending_list}_roundtrip` | Full serde JSON roundtrip for representative wire payloads |

```bash
cargo bench -p mimir-api-types --bench wire_types
```

## `mimir-client` — `sse_parser`

Added in v0.56.0 (issue #164) to cover the client-side SSE stream parser on the partial-event accumulation path (many chunks with no delimiter followed by a terminator). It compares the legacy O(n²) full-buffer rescan against the fixed cursor-based scan with a 1 MiB event cap.

| Benchmark | What it measures |
|-----------|------------------|
| `sse_accumulate/legacy_chunks_{256,1024,4096}` | Legacy O(n²) parser accumulating N chunks |
| `sse_accumulate/fixed_chunks_{256,1024,4096}` | Fixed O(n) capped parser accumulating N chunks |

```bash
cargo bench -p mimir-client --bench sse_parser
```

Indicative results: 1024 chunks 31.5 ms → 542 µs (~58×); 4096 chunks 494.9 ms → 1.21 ms (~408×).

### `mimir-core` — `pure_helpers`

| Benchmark | What it measures |
|-----------|------------------|
| `fts5_escape_mixed_inputs` | `escape_fts5` over boolean-operator/quote/unicode inputs |
| `fts5_escape_tokens_mixed_inputs` | `escape_fts5_tokens` token-AND escaping over the same inputs |
| `daily_schedule_next_after` | `DailySchedule::next_after` UTC arithmetic |
| `daily_schedule_parse` | `DailySchedule::parse("HH:MM")` × 5 |
| `job_run_status_serde_roundtrip` / `job_priority_serde_roundtrip` | Enum serde roundtrips |
| `tool_output_to_llm_text` / `tool_output_to_display_text` / `output_to_llm_text_helper` | `ToolOutput` rendering pathways |
| `config_toml_parse` | `Config` TOML deserialisation |

```bash
cargo bench -p mimir-core --bench pure_helpers
```

### `mimir-knowledge` — `pure_helpers`

| Benchmark | What it measures |
|-----------|------------------|
| `confidence_initial` | `confidence::initial` across source/connector combos |
| `confidence_inference_{20,3}_parents` | `confidence::inference_confidence` scaling |
| `confidence_default_connector_score` | Per-connector default scores |
| `memory_priority_boost` | `MemoryPriority::boost` across tiers |
| `retrieval_context_summary` | `RetrievedContext::summary` over 10 entities × 5 facts |
| `retrieval_fact_same_identity` | `RetrievedFact::same_identity` bit-pattern compare |
| `next_occurrence_mixed` | `next_occurrence` across daily/weekly/monthly/yearly |
| `memory_schema_all_facts` | `MemorySchema::all_facts` gather across 30 facts |

```bash
cargo bench -p mimir-knowledge --bench pure_helpers
```
  Uses a fixed `2024-06-15T14:30:00Z` reference time for reproducible baselines. |

## Test-suite performance baselines (v0.153.0)

The performance investigation (2026-08-26) added four benchmark suites that quantify the costs the test suite pays repeatedly, plus a script that measures the whole suite. Every open performance issue (#523–#537) names the benchmarks to watch; the fix branches must run them before/after and report the delta.

### `mimir-knowledge` — `kg_write_benchmarks`

| Benchmark | What it measures | Baseline | Issue #524 |
|-----------|------------------|----------|------------|
| `kg_schema_init` | Fresh `KnowledgeGraph::init` incl. all 58 migrations (per-test setup cost) | 65.8 ms | 62.6 ms |
| `kg_fact_insert_small_graph` | 10 fact inserts into a 6-entity graph (~0.92 ms/insert) | 9.15 ms | 7.48 ms |
| `kg_fact_insert_same_subject_growth` | 1 insert with 30 pre-existing facts on the subject (overlap-scan cost) | 2.43 ms | 2.01 ms |
| `kg_entity_create_with_aliases` | 5 entity creates with 3 aliases each | 2.41 ms | 2.22 ms |
| `kg_optimization_dedup_pass_100` | Nightly dedup pass over 100 facts (50 duplicate pairs) | 42.7 ms | 36.0 ms |
| `kg_traverse_star_300_node_cap_200` | BFS traversal of a 300-node star, cap 200 | 4.2 ms | 3.75 ms |

```bash
cargo bench -p mimir-knowledge --bench kg_write_benchmarks
```

Issue #526 follow-up on v0.153.6: `kg_fact_insert_small_graph` measured 7.48 ms and `kg_fact_insert_same_subject_growth` measured 1.94 ms on the same host, versus the original 9.15 ms and 2.43 ms baselines.

### `mimir-core` — `db_init` and `mock_llm`

| Benchmark | What it measures | Baseline |
|-----------|------------------|----------|
| `context_schema_init` | Fresh `ContextManager::new` (schema + FTS5 triggers) | 4.24 ms |
| `job_queue_schema_init` | Fresh `JobQueue::init` | 1.63 ms |
| `mock_llm_chat_call` | One `MockLlmClient::chat_message` (lock traffic) | 8.16 µs |
| `mock_llm_records_clone_100` | `chat_calls()` clone of 100 recorded calls | 34.96 µs |

```bash
cargo bench -p mimir-core --bench db_init --bench mock_llm
```

### `mimir-server` — `state_build`

| Benchmark | What it measures | Baseline |
|-----------|------------------|----------|
| `app_state_from_config_with_llm_build` | Full daemon `AppState` build (context + KG + job queue + hooks + scheduler + connectors) | 81 ms |
| `app_state_from_config_with_llm_build_shutdown` | Build + `state.shutdown()`; the 5 s is the hook-exit timeout when the dispatch loop never started (issue #536) | 5.08 s |

```bash
cargo bench -p mimir-server --bench state_build
```

### Whole-suite baseline

`scripts/perf-baseline.sh` runs the workspace suite with cargo-nextest (falling back to `cargo test --workspace` wall-time only) and prints the wall clock, the sum of per-test durations, and the 25 slowest tests.

```bash
scripts/perf-baseline.sh
```

Baseline on 2026-08-26 (cargo-nextest 0.9.143, cargo 1.97.1, debug profile): 2315 tests, 189.3 s wall, 755.9 s summed durations. The slowest tests are the connector E2E suite (12.9 s), `daemon_guard::tests::test_start_timeout` (10.4 s, issue #530), `optimization_tests::concurrent_full_runs...` (8.2 s), and `kg_traverse_tests::test_kg_traverse_node_cap` (8.0 s). Manual measurements: `mimir stop` takes 2.2 s because of a hard-coded 2 s sleep (issue #523), and `mimir-knowledge` fact-heavy seeding pays ~0.9 ms per insert (issues #526, #527).

Open performance issues tracked by these benchmarks: #523 (CLI stop sleep), #524/#525 (SQLite pragmas), #526 (fact-insert overhead), #527 (composite index), #528 (dedup O(n²)), #529 (alias batch insert), #530 (daemon-guard timeout), #531 (retry backoff), #532–#534 (fixed sleeps in tests), #535 (migration template), #536 (hook-exit timeout), #537 (supervisor poll loop).
