# How Mimir talks to its own daemon (Unix socket transport)

Mimir is one program with two roles: the **daemon** (the always-on background process that owns your data) and the **CLI** (the `mimir ask`, `mimir chat`, `mimir status`, etc. commands you type). The CLI and the daemon need a fast, secure way to talk on your own machine. Since issue #25, that connection uses a **Unix domain socket** — a special file the daemon creates in your data directory.

## What changed and why

Before, the CLI connected to the daemon over TCP (the same mechanism web servers use, at `127.0.0.1:8080`). That worked, but it could clash with other software on port 8080, gave no instant way to know whether the daemon was running, and couldn't restrict access using file permissions. A Unix socket fixes all three:

- The socket file exists only while the daemon is running — if it's there, the daemon is up; if not, it isn't.
- Only your user can connect (the file is private to you).
- Local traffic no longer needs the TCP network stack.

## How it works

- The daemon always listens on the Unix socket (default location: `~/.local/share/mimir/mimir.sock`) *and* keeps the TCP listener for remote clients and non-Unix systems.
- CLI commands prefer the socket automatically. You don't need to configure anything.
- If the daemon is not accepting connections on the socket (including a stale socket file left by a crash), the CLI knows it is not running and offers to start it.
- `mimir stop` sends the shutdown signal over the socket and waits until the socket file disappears.

## Configuration

The socket location is configurable in `~/.config/mimir/config.toml`:

```toml
[server]
# Optional override; defaults to ~/.local/share/mimir/mimir.sock on Unix.
socket_path = "~/.local/share/mimir/mimir.sock"
```

The same override is available as the `MIMIR_SERVER_SOCKET_PATH` environment variable. If you talk to a daemon on another machine, set `MIMIR_BASE_URL` (for example `http://mimir-server:8080`) and the CLI will use that instead.

## FAQ

**Do I need to change anything?** No. On Unix systems (Linux, macOS) the socket is enabled by default.

**Does Windows work?** Yes — Windows has no Unix sockets, so the daemon and CLI automatically fall back to TCP on Windows.

**Is it secure?** The socket file is owned by your user with `0600` permissions, and every request still requires the API token.

**The socket file is left over after a crash?** The daemon removes stale socket files automatically when it starts.

## Related

- Technical details: `docs/uds-transport.md`
- Daemon auto-start: `docs/wiki/daemon-auto-start.md`
- Configuration reference: `docs/wiki/configuration.md`
