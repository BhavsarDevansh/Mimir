# Graceful Shutdown

## Overview

The Mimir daemon supports graceful shutdown triggered by three signals:

1. `POST /stop` HTTP endpoint (loopback-only)
2. `SIGINT` (Ctrl-C)
3. `SIGTERM` (Unix only)

When any of these signals fire, the server enters graceful shutdown:

- `axum::serve` stops accepting new connections.
- Active requests are allowed to complete (up to a 30-second timeout).
- After the server future resolves or times out, `AppState::shutdown()` cleans up resources in order.

## Resource Cleanup Order

`AppState::shutdown()` performs the following steps:

1. **Scheduler shutdown** (`BackgroundScheduler::shutdown()`)
   - Signals the scheduler's private `watch::Sender` so the dispatch loop breaks cleanly.
   - This prevents new background jobs from starting during teardown and ensures any in-flight job's DB record is updated before the runtime drops.

2. **SQLite pool close** (`ContextManager::close()`)
   - Calls `sqlx::SqlitePool::close().await` to flush WAL and close connections.
   - After this, any further DB operations fail with a database error.

3. **LLM worker pool shutdown** (`LlmClient::shutdown()`)
   - Sends `true` on the worker pool's `shutdown_tx` watch channel.
   - Each worker task uses `tokio::select!` to race between `next_job()` and `shutdown_rx.changed()`.
   - On shutdown signal, workers break their loop, dropping their local `reqwest::Client` and closing idle HTTP connections.
   - `shutdown()` awaits each worker handle with a 5-second timeout.


## Background Task Teardown

After the server future resolves (or the 30 s timeout fires), `start_server_with_llm_and_listener` broadcasts `true` on `AppState::shutdown_tx` **before** calling `AppState::shutdown()`. This watch channel is subscribed to by every background task spawned from `start_server`:

- the config file-watcher's async relay task (which sets the `spawn_blocking` watcher's `AtomicBool` stop flag, causing it to drop the `notify` debouncer and exit within 250 ms),
- the SIGHUP reload handler,
- the condensation-notify listener.

The broadcast is sent while the runtime is still fully alive, so the tasks are guaranteed to be polled and tear down deterministically. Previously the SIGTERM/Ctrl-C path never sent on `shutdown_tx` (only `POST /stop` did), so background shutdown relied on `AppState` being dropped during runtime teardown to resolve the watchers' `shutdown_rx.changed()` via sender-drop — a race that, when lost, left the file-watcher `spawn_blocking` thread alive and deadlocked tokio's `BlockingPool::shutdown` until systemd aborted the unit with `SIGABRT`. The explicit broadcast removes that race.

## Timeout Behavior

The server future is wrapped in `tokio::time::timeout(Duration::from_secs(30), server_fut)`.

- If the server shuts down gracefully within 30 seconds, cleanup proceeds normally.
- If the timeout fires, a warning is logged (`"Graceful shutdown timed out after 30s; forcing exit."`) and resource cleanup still runs.
- The `Serve` future is dropped on timeout, immediately cutting remaining connections.

## Code References

- `mimir-server/src/lib.rs` — `shutdown_signal()` and `start_server()`
- `mimir-server/src/state.rs` — `AppState::shutdown()`
- `mimir-core/src/scheduler.rs` — `BackgroundScheduler::shutdown()`
- `mimir-core/src/context.rs` — `ContextManager::close()`
- `mimir-core/src/llm/pool.rs` — `LlmWorkerPool::shutdown()`
- `mimir-core/src/llm/client.rs` — `LlmClient::shutdown()`
- `mimir-core/src/llm/backend.rs` — `LlmBackend::shutdown()` trait method
