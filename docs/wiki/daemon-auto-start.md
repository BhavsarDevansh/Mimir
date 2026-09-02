# Daemon Auto-Start

When you run a Mimir client command (`ask`, `chat`, `status`, `memory`, or `stop`), the CLI first checks whether the Mimir daemon is running. If the daemon is not reachable, Mimir can optionally start it for you.

## What Happens When the Daemon Is Down

The CLI probes the daemon with a lightweight check: a 500 ms connection attempt on the Unix socket when it talks over the socket, or the `/health` endpoint over TCP (a cheap liveness check that never touches the LLM or database). The transport is resolved from `MIMIR_BASE_URL`, then the Unix socket (`MIMIR_SERVER_SOCKET_PATH` → `server.socket_path` → `<data_dir>/mimir.sock`), then `server.bind_addr` in your config (with `0.0.0.0` normalised to loopback), then the default `http://127.0.0.1:8080`. If the daemon is not responding, you will see:

```
Error: Mimir is not running.
Start the server now? [y/N]:
```

## How to Answer the Prompt

- **Type `y` or `yes` and press Enter** — Mimir will spawn the daemon in the background and wait for it to become ready.
- **Type `n`, press Enter, or send EOF (Ctrl+D)** — The command aborts with a clear error.

If you approve, Mimir:

1. Spawns `mimir start` using the same binary you are currently running.
2. Daemon stdout and stderr are redirected to null so it runs silently in the background.
3. Polls the same transport probe (socket connection or `/health`) every 200–1 000 ms with exponential backoff.
4. Proceeds with your original command as soon as the daemon is ready.

The total wait time is capped at **10 seconds**. If the daemon has not started by then, the command exits with a timeout error. Automated tests inject a shorter timeout without changing this normal behaviour.

## Skipping the Prompt

The prompt only appears when the daemon is down. You can avoid it entirely by keeping the daemon running:

```bash
# Start the daemon manually (runs in the foreground)
mimir start

# Or use systemd, screen, tmux, etc. to keep it running in the background
```

In non-interactive environments (CI, scripts, piped input), the prompt reads from stdin. If stdin does not contain `y` or `yes`, the command aborts cleanly. For automated workflows, start the daemon explicitly before invoking client commands:

```bash
mimir start &
sleep 2
mimir ask "Hello world"
```

## What Commands Are Protected

The guard runs before:

- `mimir ask`
- `mimir chat`
- `mimir status`
- `mimir memory`
- `mimir stop`

Commands that do **not** need the daemon and therefore skip the guard:

- `mimir init`
- `mimir start`
- `mimir tool`
- `mimir skill`

## Troubleshooting

**The prompt never appears and the command just hangs.**
- Ensure the `mimir` executable is in your `PATH` and the system can locate it.
- If you are using a symlink, the symlink target must be a valid executable.

**The daemon starts but the command still times out.**
- The 10-second timeout may be too short for slow systems. Start the daemon manually (`mimir start`) before running client commands.

**I see "prompt error: declined" in CI logs.**
- The daemon guard read an empty or non-yes response from stdin. Start the daemon explicitly in your CI pipeline.

## See Also

- [Server](../server.md) — How `mimir start` works and how to run it as a systemd service.
- [CLI Commands](cli-commands.md) — Full list of client and management commands.
