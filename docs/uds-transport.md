# Unix Domain Socket Transport (issue #25)

> **Status:** Implemented. The daemon serves the same Axum router on both a TCP listener and a Unix domain socket; an explicit `MIMIR_BASE_URL` wins for remote daemons, otherwise local CLI commands prefer the socket and fall back to TCP for remote daemons and Windows.

## Why a Unix socket?

Local CLI↔daemon traffic previously used only TCP (`127.0.0.1:8080`), which has four drawbacks that the Unix socket removes: port conflicts with unrelated processes, no filesystem-level access control, no instant daemon detection (a local connection attempt on the socket, with no HTTP round trip), and needless TCP overhead for loopback traffic.

## Transport selection

The CLI resolves its transport once per invocation, in this order:

1. `MIMIR_BASE_URL` — an explicit remote or alternate daemon wins over the local socket.
2. Unix socket: `MIMIR_SERVER_SOCKET_PATH` → `server.socket_path` in `config.toml` → the platform default `<data_dir>/mimir.sock` (on Unix). `~` is expanded to the home directory.
3. TCP fallback: the configured `server.bind_addr` (loopback-normalised) or the compiled default `http://127.0.0.1:8080`.

The daemon resolves its listener the same way, so on Unix the daemon and CLI agree on the socket location without configuration: the socket is enabled by default at `<data_dir>/mimir.sock`. Windows is TCP-only, so the daemon does not listen on a Unix socket there.

## Daemon side

- `start_server_with_llm` binds the TCP listener as before, then binds the Unix socket (creating the parent directory; an existing socket file is removed only after a bounded 500 ms connect attempt proves no live daemon is listening, and a live socket aborts startup with an "already in use" error instead of being unlinked and stolen).
- The socket file is chmod'ed `0600` so only the owning user can connect; filesystem permissions are the transport-level access control.
- Both listeners serve the same router. A custom connect-info type (`LocalPeer`) replaces `ConnectInfo<SocketAddr>` so the loopback guard and `/stop` peer attribution work identically on both transports: a Unix peer is always local, while TCP peers must still be loopback addresses.
- The socket server observes the same shutdown trigger as the TCP server. The socket file is removed by the listener task itself while it still owns the pathname; after an abort, the caller removes it only once a probe confirms no live daemon took the pathname over. The next startup removes any file left by a hard crash once it proves the file is stale.
- Config reload keeps `server.socket_path` behind the sensitive-field gate (changing it requires a restart).

## Client side

`mimir-client` builds a `reqwest` client with `ClientBuilder::unix_socket` (reqwest 0.13; no `hyperlocal` dependency needed) when constructed with the UDS constructors (`new_uds`, `try_new_uds`, `with_token_uds`, `try_new_with_token_uds`). The base URL for UDS clients is `http://localhost` — the host is never contacted because the socket replaces the connection entirely.

Daemon detection is instant: the CLI attempts a connection on the socket — a local syscall with no HTTP round trip — bounded at 500 ms by the daemon guard, which keeps the TCP `GET /health` probe as the fallback for the remote/Windows transport. Liveness is decided by the connection attempt, never by socket-file existence: a connection succeeds only while the daemon is listening, so a stale socket file left by a crashed daemon is detected as down and the daemon guard auto-starts it. `mimir stop` verifies shutdown by observing that the socket file disappears.

If the socket cannot be bound (a bad configured path, missing permissions), the daemon fails to start with a descriptive error rather than silently continuing TCP-only — the CLI would otherwise keep targeting a socket that can never exist.

## Tests

- `mimir-core`: socket-path resolution (configured value, tilde expansion, default fallback) and env-over-config precedence.
- `mimir-server`: full-daemon integration tests over a Unix socket — `/health` round trip, `/stop` over the socket (loopback guard accepts Unix peers), socket file removal on graceful shutdown, and stale-socket recovery before binding. Test daemons isolate the socket inside their temp dir so parallel suites never fight over the default path.
- `mimir` (CLI): transport precedence (`MIMIR_BASE_URL` wins; configured path; default socket), daemon-guard connect-based probe (live / missing / stale socket files), and token-bearing UDS client construction.

## Configuration reference

| Setting | Default on Unix | Default on Windows |
|---------|-----------------|--------------------|
| `server.socket_path` / `MIMIR_SERVER_SOCKET_PATH` | `<data-dir>/mimir.sock` | disabled (TCP only) |

Setting `server.socket_path` (or the env var) to a custom path overrides the default. There is no way to disable the socket on Unix: it is the primary local transport.
