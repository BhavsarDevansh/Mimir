# Graceful Shutdown

## Overview

The Mimir daemon supports graceful shutdown triggered by three signals:

1. `POST /stop` HTTP endpoint (loopback-only)
2. `SIGINT` (Ctrl-C)
3. `SIGTERM` (Unix only)

When any of these signals fire, the server enters graceful shutdown:

- `axum::serve` stops accepting new connections.
- In-flight requests are allowed to drain for up to `GRACEFUL_DRAIN_TIMEOUT` (30 s).
- After the drain completes (or the drain bound elapses), `AppState::shutdown()` cleans up resources in order.

> **Serving lifetime is unbounded.** The daemon runs indefinitely while no shutdown is requested. The 30-second bound applies **only** to the post-signal drain phase — it is not a lifetime/idle timeout. An earlier implementation wrapped the entire server future in `tokio::time::timeout`, which caused the daemon to self-terminate 30 s after start; the two-phase `serve_with_bounded_drain` fixes this.

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

After the drain completes (or the drain bound elapses), `start_server_with_llm_and_listener` broadcasts `true` on `AppState::shutdown_tx` **before** calling `AppState::shutdown()`. This watch channel is subscribed to by every background task spawned from `start_server`:

- the config file-watcher's async relay task (which sets the `spawn_blocking` watcher's `AtomicBool` stop flag, causing it to drop the `notify` debouncer and exit within 250 ms),
- the SIGHUP reload handler,
- the condensation-notify listener.

The broadcast is sent while the runtime is still fully alive, so the tasks are guaranteed to be polled and tear down deterministically. Previously the SIGTERM/Ctrl-C path never sent on `shutdown_tx` (only `POST /stop` did), so background shutdown relied on `AppState` being dropped during runtime teardown to resolve the watchers' `shutdown_rx.changed()` via sender-drop — a race that, when lost, left the file-watcher `spawn_blocking` thread alive and deadlocked tokio's `BlockingPool::shutdown` until systemd aborted the unit with `SIGABRT`. The explicit broadcast removes that race.

## Trigger Architecture

There is exactly **one** OS-signal listener per process. `serve_with_bounded_drain` spawns `spawn_os_signal_shutdown`, a dedicated task that races `ctrl_c()` and `SIGTERM` (Unix) and, on either, sends `true` on the shared `shutdown_tx` watch channel — the same channel the `/stop` endpoint writes to. Both axum's `with_graceful_shutdown` future and the phase-1 serving loop observe that channel through `watch_shutdown`, which first inspects the current watch value via `borrow_and_update()` and only then awaits `changed()`. This guards against the subscription race: because `spawn_os_signal_shutdown` is spawned **before** `graceful_rx`/`trigger_rx` are created with `subscribe()`, a SIGTERM/Ctrl-C arriving in that gap sends `true` before any receiver exists. A freshly subscribed receiver's `changed()` only wakes on *future* updates, so without the upfront value check the already-fired trigger would be missed and the server would wait indefinitely. Checking the current value first means an already-fired trigger returns immediately, while later triggers are still caught by `changed()`.

This avoids the previous race where two independent `shutdown_signal` futures each built their own `ctrl_c()`/`SIGTERM` listeners: the phase-1 waiter could observe a signal before axum's graceful-shutdown future had registered interest, leaving axum still accepting connections until the drain bound kicked in. With a single listener fanning into one shared trigger, both phases fire in lockstep.

The OS signal handlers are registered **synchronously**, before the listener task is spawned: `tokio::signal::unix::signal()` installs the libc handler in its constructor, so a SIGTERM/SIGINT arriving before the spawned task is first polled is caught instead of hitting the default disposition and killing the process. This closes the startup race (issue #329) where the health listener became ready before the signal task was scheduled — under parallel load the window was wide enough for a SIGTERM sent right after readiness to terminate the daemon with the default disposition (exit status = signal 15) instead of taking the graceful path.

## Shutdown Trigger Attribution

Every code path that fires the shared `shutdown_tx` watch trigger now logs the **cause** of the shutdown *before* sending, via the `ShutdownSource` enum:

| Variant | Origin | Log line (example) |
|---|---|---|
| `StopEndpoint(SocketAddr)` | `POST /stop` endpoint (e.g. `mimir stop`) | `Shutdown requested via /stop endpoint from 127.0.0.1:45678.` |
| `Terminate` | `SIGTERM` | `Shutdown triggered by SIGTERM (signal).` |
| `Interrupt` | `Ctrl-C` / `SIGINT` | `Shutdown triggered by interrupt (Ctrl-C).` |

`ShutdownSource::attribution()` returns the human-readable line; the `/stop` handler additionally captures the requesting peer's `SocketAddr` through an axum `ConnectInfo` extractor (loopback-guaranteed by the `require_loopback` middleware).

This was added because all three paths previously emitted the identical line `Server shut down gracefully.`, which recorded *that* the daemon stopped but not *what* stopped it. An unexplained stop on 2026-06-30 (systemd recorded no `Stopping`/`Stopped` lifecycle line, so the trigger came from within the process) was impossible to attribute from the journal.

### Untriggered exits are no longer mislabelled

`serve_with_bounded_drain` distinguishes two exit outcomes through the pure `server_exit_message(triggered: bool)` helper:

- **`true`** — the shared trigger fired first (Phase 2 drain completed): `Server shut down gracefully.`
- **`false`** — the server future resolved on its own *without* any trigger firing first (e.g. a fatal listener error), logged at `warn!` level as `Server future resolved without a shutdown trigger; exiting.`

Previously both outcomes logged `Server shut down gracefully.`, so a non-graceful server exit was misreported as graceful and masked the real cause. `server_exit_message` is a pure function so the "untriggered ≠ graceful" invariant is unit-tested without capturing log output.

## Timeout Behavior

Shutdown is split into two phases by `serve_with_bounded_drain`:

1. **Serve (unbounded)** — the server future is polled concurrently with `watch_shutdown(trigger_rx)`, which completes when the shared `shutdown_tx` watch trigger fires (Ctrl-C, SIGTERM, or `/stop`). The server keeps accepting and handling connections until a trigger fires. If the server future resolves on its own (e.g. a fatal listener error), it is propagated immediately.
2. **Drain (bounded to `GRACEFUL_DRAIN_TIMEOUT` = 30 s)** — once the trigger fires, axum's own `watch_shutdown(graceful_rx)` has fired too (same trigger), so it has stopped accepting and is draining in-flight connections. `tokio::time::timeout` bounds **only** this drain:
   - If the drain completes within 30 s, cleanup proceeds normally.
   - If the drain bound elapses, a warning is logged (`"Graceful drain timed out after 30s; forcing exit."`) and the pinned server future is dropped, immediately cutting remaining connections.

Resource cleanup (`AppState::shutdown()`) runs in either case.

## Code References

- `mimir-server/src/shutdown.rs` — `ShutdownSource`, `server_exit_message()`, `spawn_os_signal_shutdown()`, `watch_shutdown()`, `serve_with_bounded_drain()`, and `GRACEFUL_DRAIN_TIMEOUT`
- `mimir-server/src/server.rs` — `start_server()` and the daemon startup sequence
- `mimir-server/src/routes/stop.rs` — `stop_handler()` (logs `ShutdownSource::StopEndpoint` with the peer address)
- `mimir-server/src/state/` — `AppState::shutdown()`
- `mimir-core/src/scheduler.rs` — `BackgroundScheduler::shutdown()`
- `mimir-core/src/context/` — `ContextManager::close()`
- `mimir-core/src/llm/pool/` — `LlmWorkerPool::shutdown()`
- `mimir-core/src/llm/client/` — `LlmClient::shutdown()`
- `mimir-core/src/llm/backend.rs` — `LlmBackend::shutdown()` trait method
