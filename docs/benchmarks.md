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

### `memory_manager`

| Benchmark | What it measures |
|-----------|------------------|
| `memory_add_small` | Append 11 bytes to `memory.md` |
| `memory_add_large` | Append 5000 bytes to `memory.md` |
| `memory_replace` | In-place string replacement with disk write |
| `memory_usage_calculation` | `chars().count()` and percentage math |

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
