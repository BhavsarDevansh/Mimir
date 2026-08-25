# Daemon Guard

The daemon guard is a shared helper that ensures the Mimir HTTP server is running before any client-mode command attempts to communicate with it.

## Module

- `mimir/src/daemon_guard.rs`

## Public API

```rust
pub async fn ensure_daemon_running(
    base_url: &str,
    already_tried: &mut bool,
) -> Result<(), DaemonGuardError>;
```

### Parameters

- `base_url` — The daemon HTTP API base URL, e.g. `http://127.0.0.1:8080`. The helper probes `{base_url}/health`. Resolved from `MIMIR_BASE_URL` → `server.bind_addr` (config) → compiled default; see [CLI base URL](#cli-base-url) below.
- `already_tried` — A shared mutable flag passed from `main.rs`. It guarantees that **at most one** auto-start attempt is made per CLI invocation, even if multiple client commands were (hypothetically) triggered.

### Error Variants (`DaemonGuardError`)

| Variant | Cause |
|---------|-------|
| `Prompt` | User declined the prompt, or stdin returned EOF / an IO error. |
| `Spawn` | `std::env::current_exe()` failed, or the OS refused to spawn the child process. |
| `StartTimeout` | The daemon did not respond to `GET /health` within the 10 s post-spawn polling window. |
| `Connection` | Reserved for unexpected probe-level failures (currently unused). |

## Detection Flow

1. **Fast probe** — A dedicated `reqwest` client with a 500 ms total-request timeout sends `GET {base_url}/health`. `health` is a cheap liveness endpoint that never touches the LLM backend or database, so a healthy-but-slow provider cannot make the probe time out. If the response is HTTP 2xx, the helper returns `Ok(())` immediately.
2. **Prompt** — If the probe fails (connection refused, timeout, or non-2xx), the helper prints:
   ```text
   Error: Mimir is not running.
   Start the server now? [y/N]:
   ```

   It then reads a single line from `stdin`. The line is trimmed and lowercased. Only `y` or `yes` are accepted. EOF, empty input, or anything else aborts with a `Prompt` error.
3. **Spawn** — On approval, the helper calls `std::env::current_exe()` to locate the current `mimir` binary and spawns it as `mimir start`. Stdout and stderr are redirected to null so the daemon runs silently in the background.
4. **Polling** — The helper enters a poll loop:
   - Initial delay: **200 ms**
   - After each failed probe, delay doubles: 200 → 400 → 800 → **capped at 1 000 ms**
   - Total wall-clock budget: **≤ 10 s**
   - On the first successful `GET /health`, return `Ok(())`.
   - If the 10 s budget expires, return `Err(DaemonGuardError::StartTimeout)`.

## Design Decisions

- **Why a separate `reqwest` client?** The existing `mimir_client::MimirClient` is configured with a 10 s connect timeout and 120 s request timeout. A 500 ms fast probe requires its own short-timeout client.
- **Why `std::process::Command` instead of `tokio::process::Command`?** The spawn is fire-and-forget; we do not await the child. The synchronous `std` API is simpler and avoids adding the `process` feature to `tokio` in production.
- **Why pass `already_tried` explicitly?** A `static AtomicBool` would be global, which complicates parallel test execution. An explicit `bool` parameter makes the dependency visible and trivially testable.
- **Why trait-based internals?** `Probe`, `PromptReader`, and `ProcessSpawner` are internal traits that let unit tests inject mock behaviour without relying on real HTTP servers or interactive stdin. The public API remains a plain async function.

## CLI base URL

The daemon guard's transport is resolved per invocation in `mimir/src/transport.rs` (issue #25): an explicit `MIMIR_BASE_URL` wins, then the Unix domain socket (`MIMIR_SERVER_SOCKET_PATH` → `server.socket_path` → `<data_dir>/mimir.sock`), then TCP via `mimir/src/constants.rs` (`MIMIR_BASE_URL` → `server.bind_addr`, with wildcard hosts like `0.0.0.0` normalised to loopback → compiled default `http://127.0.0.1:8080`). On Unix the probe is a 500 ms connect attempt on the socket — a local syscall with no HTTP round trip that succeeds only while the daemon is listening, so a stale socket file left by a crashed daemon is detected as down and the guard auto-starts it; on TCP it is the `GET /health` probe. This means the CLI automatically targets whichever transport the daemon listens on, so a non-default `server.bind_addr` no longer causes the guard to probe the wrong port and spuriously prompt to start an already-running daemon.

## Test Coverage

Unit tests in `mimir/src/daemon_guard.rs` cover:

| Test | Scenario |
|------|----------|
| `test_already_running` | Daemon is up on first probe; returns `Ok(())` without prompting or spawning. |
| `test_prompt_yes_spawns_and_polls` | Daemon is down; user types `y`; mock spawn succeeds; poll eventually succeeds. |
| `test_prompt_no` | Daemon is down; user types `n`; returns `Prompt` error. |
| `test_prompt_eof` | Daemon is down; stdin is empty/EOF; returns `Prompt` error. |
| `test_spawn_failure` | User types `y`; mock spawn fails; returns `Spawn` error. |
| `test_start_timeout` | User types `y`; mock spawn succeeds; daemon never becomes ready; returns `StartTimeout`. |
| `test_already_tried_skips_prompt` | `already_tried` is `true` on entry; skips prompt and returns `StartTimeout`. |

Additionally, `mimir/tests/cli_tests.rs` contains binary-level integration tests that verify the daemon guard fires when the server is not running.

## Related Documentation

- [`docs/wiki/daemon-auto-start.md`](wiki/daemon-auto-start.md) — User-facing guide for the auto-start prompt.
- [`mimir/src/main.rs`](../mimir/src/main.rs) — Where `ensure_daemon_running` is invoked before each client command.
