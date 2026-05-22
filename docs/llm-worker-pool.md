# LLM Worker Pool

## Overview

`LlmWorkerPool` (`mimir-core/src/llm/pool.rs`) is a priority-based work distributor that ensures user-facing chat requests are always serviced before background system tasks. All LLM calls made via `LlmClient` now pass through this pool.

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
4. Process the job using a direct `LlmClient` (no pool recursion).

### Backpressure

When a queue is at capacity, `enqueue_chat()` (and its streaming variant) returns `LlmError::QueueFull`. Callers should translate this to an HTTP `503 Service Unavailable` with a `Retry-After` header.

## Job Types

```rust
pub enum Job {
    Chat { messages, respond: oneshot::Sender<Result<(String, Usage), LlmError>> },
    ChatStream { messages, respond: mpsc::Sender<Result<StreamItem, LlmError>> },
}
```

## Client Integration

`LlmClient` is the public enqueue interface:

- `LlmClient::new(config)` — creates a default pool with 1 worker.
- `LlmClient::with_pool(pool)` — injects a custom pool (useful in tests).
- `LlmClient::new_direct(config)` — crate-internal constructor that bypasses the pool (used by workers).

## Future Configuration

In a future issue, `WorkerPoolConfig` will be loaded from `Config::llm` so users can tune queue sizes and worker counts via `config.toml`.
