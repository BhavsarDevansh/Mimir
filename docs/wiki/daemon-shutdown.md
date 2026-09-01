# Daemon Shutdown

## How It Works

The Mimir daemon can be shut down gracefully in three ways:

- **CLI command**: `mimir stop` sends an HTTP signal to the daemon and polls for up to 5 seconds to verify it has exited.
- **Ctrl-C**: Pressing `Ctrl-C` while the daemon is running in the foreground triggers graceful shutdown.
- **SIGTERM**: On Unix systems, sending `SIGTERM` (e.g. from `systemctl stop mimir` or `kill -TERM`) also triggers graceful shutdown.

## `mimir stop`

```bash
mimir stop
```

What happens:

1. The CLI checks if the daemon is reachable on the configured transport.
2. If unreachable, it prints `Mimir is not running.` to stderr and exits with code `1`.
3. If reachable, it sends `POST /stop` to the daemon.
4. It polls reachability every 100 ms, returning as soon as the daemon is unreachable.
5. If the daemon is no longer reachable, it prints `Mimir daemon stopped.` and exits `0`.
6. If the daemon is still reachable after 5 seconds, it prints a warning to stderr and exits `1`.

## What Graceful Shutdown Means

When shutdown is triggered:

- The server stops accepting new HTTP connections.
- In-flight requests are allowed to finish (within a 30-second drain limit).
- The SQLite database pool is closed, flushing any pending writes.
- LLM worker threads are stopped and their HTTP connections closed.

The daemon runs indefinitely while no shutdown is requested — there is no idle or lifetime timeout. The 30-second limit bounds only the drain of in-flight requests after a shutdown signal (Ctrl-C, `SIGTERM`, or `mimir stop`).

## SIGTERM and systemd

`systemctl --user stop mimir` sends `SIGTERM`. The daemon catches it, drains in-flight requests (up to 30 s), tears down all background tasks (config file-watcher, SIGHUP handler, hooks dispatch loop), and exits promptly — well within systemd's `TimeoutStopSec`. Previously the `SIGTERM` path could hang during runtime teardown (deadlocking the tokio blocking pool until systemd aborted the unit with `SIGABRT`); that is fixed by an explicit shutdown broadcast before the runtime drops. Error paths that never reach that broadcast (a crash or startup failure that drops the runtime early) are covered too: the config watcher's blocking thread is tied to its async task through a lifetime channel, so it always exits during runtime teardown and the process can never hang on the blocking-pool join (issue #415).

The signal handlers are installed before the daemon starts accepting connections, so a `SIGTERM` or `Ctrl-C` arriving during startup (for example, `systemctl stop` racing a slow start) is handled gracefully instead of killing the process with the default signal disposition.

## Finding Out Why the Daemon Stopped

The daemon now logs the **cause** of every shutdown to the journal, just before it stops, so an unexplained stop can be diagnosed instead of guessed at:

- `mimir stop` / `POST /stop` → `Shutdown requested via /stop endpoint from <peer>.`
- `Ctrl-C` / `SIGINT`        → `Shutdown triggered by interrupt (Ctrl-C).`
- `SIGTERM`                  → `Shutdown triggered by SIGTERM (signal).`
- The server exited on its own (no trigger) → `Server future resolved without a shutdown trigger; exiting.` (logged as a warning)

To investigate an unexpected stop:

```bash
journalctl --user -u mimir -n 50 --no-pager
```

Look for the attribution line immediately above `Server shut down gracefully.`. If you instead see `Server future resolved without a shutdown trigger`, the daemon exited for a reason other than a stop request (e.g. a listener error) — report it.

> Tip: if you want the daemon to come back automatically after a graceful stop (e.g. across logouts), set `Restart=always` in the systemd unit and run `loginctl enable-linger <user>`. With the default `Restart=on-failure`, a *clean* stop is not restarted.

## Best Practices

- **Prefer `mimir stop` over `kill -9`**: The CLI verifies the daemon actually exits.
- **Use `SIGTERM` for systemd**: The daemon handles `SIGTERM` correctly, so systemd service units should use the default `ExecStop` behavior.
- **Avoid `kill -9` unless the daemon is stuck**: A force kill bypasses cleanup and may leave the SQLite WAL unflushed.
