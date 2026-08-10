# LLM Worker Pool

## Overview

`LlmWorkerPool` (`mimir-core/src/llm/pool/`) is a priority-based work distributor that ensures user-facing chat requests are always serviced before background system tasks. All LLM calls made via `LlmClient` now pass through this pool.

## Design

### Queues

Two bounded `tokio::sync::mpsc`-style queues (implemented via `Mutex<VecDeque>` + `Notify` for multi-worker safety):

- **User queue** — highest priority. Used for all user-facing chat and streaming requests.
- **System queue** — lower priority. Reserved for background tasks, maintenance, and investigations.

### Workers

`WorkerPoolConfig` controls pool behaviour:

```rust
pub struct WorkerPoolConfig {
    pub worker_threads: u8,   // default: 1
    pub user_queue_size: u16, // default: 100
    pub system_queue_size: u16, // default: 100
}
```

Each worker loops:

1. Pop from user queue.
2. If user queue is empty, pop from system queue.
3. If both are empty, wait on `Notify`.
4. Increment the `in_flight` counter, process the job using a direct `LlmClient`, then decrement the counter on completion (including on panic — a drop guard guarantees the decrement).
5. The counter is exposed via `LlmBackend::in_flight_count()` so the scheduler can gate background work on true pool idleness.

### Backpressure

When a queue is at capacity, `enqueue_chat()` (and its streaming variant) returns `LlmError::QueueFull`. Callers should translate this to an HTTP `503 Service Unavailable` with a `Retry-After` header.

## Constructor Safety

`LlmWorkerPool::new` is **all-or-nothing**: it builds every worker's `reqwest`-backed
`LlmClient` up front into a `Vec` and only spawns worker tasks once *all* clients have
succeeded. If any `LlmClient::new_direct` fails, `new` returns `Err` with no worker tasks
spawned, so no detached/orphaned tasks are left behind. This avoids the partial-startup
hazard where a later-iteration build failure would leave earlier workers spawned with
no `LlmWorkerPool` handle to signal shutdown.

A successful `new` registers exactly `worker_threads` join handles, each joined by
`shutdown()`. The regression test `test_pool_spawns_exactly_configured_workers` guards
this invariant.

## Job Types

```rust
pub enum Job {
    Chat { messages, respond: oneshot::Sender<Result<(String, Usage), LlmError>> },
    ChatStream { messages, respond: mpsc::Sender<Result<StreamItem, LlmError>> },
}
```

## Client Integration

`LlmClient` is the public enqueue interface:

- `LlmClient::new(config)` — async constructor that creates a default pool with 1 worker (spawned inside the Tokio runtime).
- `LlmClient::with_pool(pool)` — injects a custom pool (useful in tests).
- `LlmClient::new_direct(config)` — crate-internal constructor that bypasses the pool (used by workers).

## Future Configuration

In a future issue, `WorkerPoolConfig` will be loaded from `Config::llm` so users can tune queue sizes and worker counts via `config.toml`.
