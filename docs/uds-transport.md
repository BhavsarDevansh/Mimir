# Unix Domain Socket Transport (issue #25)

> **Status:** Implemented. The daemon serves the same Axum router on both a TCP listener and a Unix domain socket; local CLI commands prefer the socket and fall back to TCP for remote daemons (`MIMIR_BASE_URL`) and Windows.

## Why a Unix socket?

Local CLI↔daemon traffic previously used only TCP (`127.0.0.1:8080`), which has four drawbacks that the Unix socket removes: port conflicts with unrelated processes, no filesystem-level access control, no instant daemon detection (a socket file exists only while the daemon is running), and needless TCP overhead for loopback traffic.

## Transport selection

The CLI resolves its transport once per invocation, in this order:

1. `MIMIR_BASE_URL` — an explicit remote or alternate daemon wins over the local socket.
2. Unix socket: `MIMIR_SERVER_SOCKET_PATH` → `server.socket_path` in `config.toml` → the platform default `<data_dir>/mimir.sock` (on Unix). `~` is expanded to the home directory.
3. TCP fallback: the configured `server.bind_addr` (loopback-normalised) or the compiled default `http://127.0.0.1:8080`.

The daemon resolves its listener the same way, so the daemon and CLI always agree on the socket location without configuration: on Unix the socket is enabled by default at `<data_dir>/mimir.sock`. On Windows, Unix sockets are unavailable and both sides use TCP.

## Daemon side

- `start_server_with_llm` binds the TCP listener as before, then binds the Unix socket (creating the parent directory and removing any stale socket file left by a crashed daemon before binding).
- The socket file is chmod'ed `0600` so only the owning user can connect; filesystem permissions are the transport-level access control.
- Both listeners serve the same router. A custom connect-info type (`LocalPeer`) replaces `ConnectInfo<SocketAddr>` so the loopback guard and `/stop` peer attribution work identically on both transports: a Unix peer is always local, while TCP peers must still be loopback addresses.
- The socket server observes the same shutdown trigger as the TCP server; the socket file is removed once the Unix server task is joined (and when it is aborted after a fatal TCP error). The next startup removes any stale file left by a hard crash.
- Config reload keeps `server.socket_path` behind the sensitive-field gate (changing it requires a restart).

## Client side

`mimir-client` builds a `reqwest` client with `ClientBuilder::unix_socket` (reqwest 0.13; no `hyperlocal` dependency needed) when constructed with the UDS constructors (`new_uds`, `try_new_uds`, `with_token_uds`, `try_new_with_token_uds`). The base URL for UDS clients is `http://localhost` — the host is never contacted because the socket replaces the connection entirely.

Daemon detection is instant: the CLI attempts a connection on the socket — a local syscall with no HTTP round trip. A connection succeeds only while the daemon is listening, so a stale socket file left by a crashed daemon is correctly detected as down and the daemon guard auto-starts it. `mimir stop` verifies shutdown by observing that the socket file disappears.

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
