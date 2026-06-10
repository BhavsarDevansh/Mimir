# Benchmarks

## Overview

Criterion `0.8.2` with the `async_tokio` feature is used for all benchmarks.
Benchmarks are located in `mimir-core/benches/` and compiled as standalone
binaries (`harness = false`).

## Design Principles

- **Stateful ops use `iter_batched`**: Database and file-backed benchmarks
  create fresh state per iteration to avoid cross-run contamination.
- **`std::hint::black_box`**: Prevents the compiler from optimising away pure
  computations (e.g. `usage_pct()`).
- **Synchronous runner for nested async**: `b.iter_batched` with an explicit
  `rt.block_on(...)` avoids "Cannot start a runtime from within a runtime" panics
  that occur when `rt.block_on` is called inside a `b.to_async(&rt)` closure.

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
## `mimir-knowledge`

| Benchmark | What it measures |
|-----------|------------------|
| `entity_resolution_exact` | Exact name lookup by `get_by_name` with 10k facts |
| `entity_resolution_alias` | Alias lookup with 10k facts |
| `fts5_search` | FTS5 full-text search over entities with 10k facts |
| `facts_by_subject_with_chain` | Retrieve facts by subject with a 3-hop chain present |
| `inference_chain_100` | Transitivity inference across a 100-fact `is_in` chain |
| `memory_condensation` | Build and render ranked memory schema from 10k facts |

```bash
# Run all knowledge graph benchmarks
cargo bench -p mimir-knowledge

# Run a single benchmark
cargo bench -p mimir-knowledge --bench kg_benchmarks -- entity_resolution_exact
```
