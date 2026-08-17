# API Authentication

## Overview

The daemon HTTP API is protected by a local bearer token (issue #281). Every route except `GET /health` rejects requests that do not present the token as `Authorization: Bearer <token>` with `401 Unauthorized` and a `WWW-Authenticate: Bearer` challenge. The token is a shared secret between the daemon and the `mimir` CLI, generated automatically on first use, so CLI commands keep working unmodified after `mimir init`.

## Threat Model

Before this change the API had no authentication: the loopback guard (`require_loopback`) only restricted a subset of routes to loopback callers, and loopback is not an auth boundary. Any local process — or any other local user on a multi-user machine — could read the entire knowledge graph, forge chat turns, edit or forget facts, and delete connectors. A `server.bind_addr = "0.0.0.0:8080"` configuration exposed the unguarded routes to the network with no protection at all; the loopback-gated routes stayed local-only.

The bearer token closes that gap: without the token, the knowledge graph is unreadable and unmutable, regardless of whether the caller is local or remote. The loopback guard remains as a second, independent layer for destructive routes, and the daemon logs a warning at startup when the bind address is not loopback (the token is then the only authentication for remote callers).

## Token Lifecycle

- **Location:** `~/.local/share/mimir/api_token` (resolved via `mimir_core::paths::data_dir`, so `XDG_DATA_HOME` is honoured).
- **Generation:** 32 random bytes (256 bits) from the operating system's CSPRNG (`getrandom`), hex-encoded to 64 characters. Written to a temporary file and published atomically (hard link) with mode `0600` (Unix), so a concurrent creation race resolves to whichever process published first and both processes then read the same complete token.
- **Creation points:** `mimir init` (best-effort), daemon startup (`mimir_server::start_server`), and the first CLI command (`mimir::cli_util::make_client`). Existing installs are upgraded transparently the next time the daemon or CLI runs.
- **Existing files are never overwritten:** a user-supplied token is preserved. On Unix, a token file with group/other permissions triggers a warning log (the token is only as secret as the file that stores it).
- **Rotation:** delete the file and restart the daemon (or run any CLI command) to generate a fresh token. The daemon and CLI must agree on the token, so rotate while the daemon is stopped, or restart the daemon afterwards.

## Enforcement

- `mimir-server/src/app.rs` assembles the router as a protected sub-router (every route except `GET /health`) wrapped in `require_auth` via `axum::middleware::from_fn_with_state`, merged with the unauthenticated `/health` route.
- `require_auth` extracts the `Authorization` header, strips the `Bearer` prefix, and compares the presented token against `AppState::api_token` in constant time (`mimir_core::auth::verify_api_token`, backed by the `subtle` crate), so comparison time does not depend on the matching prefix.
- `GET /health` stays unauthenticated on purpose: it is the daemon-guard liveness probe and reveals nothing beyond "a daemon is listening". A token-bearing probe would make a daemon with a different token look "down" and trigger a second daemon start.
- The loopback guard is unchanged and runs inside the auth layer, so a non-loopback caller without the token gets `401` and a non-loopback caller with the token gets `403` on loopback-gated routes.

## Client Behaviour

- `MimirClient::with_token` / `MimirClient::try_new_with_token` build a `reqwest` client whose default headers include `Authorization: Bearer <token>`, so every request — including SSE streams — carries the token. `MimirClient::new` / `try_new` remain tokenless for tests and mock servers.
- The CLI's `make_client` helper loads (or creates) the token from the data dir and uses the token-bearing constructor. If the token cannot be loaded it warns and falls back to a tokenless client, so the daemon's `401` surfaces the problem instead of a silent failure.
- The daemon guard probes `GET /health` without a token, so the auto-start flow is unaffected.

## Non-Loopback Binds

Binding to `0.0.0.0:8080` (or any non-loopback address) exposes the API to the network. With authentication enabled the token is the only authentication for such a bind (loopback-gated routes still return `403` to remote callers), so treat the token file like a password: keep it `0600`, do not copy it to other machines, and rotate it if it leaks. The daemon logs a warning at startup when the bound address is not loopback. For LAN use, prefer a reverse proxy with TLS and its own authentication in front of the daemon (see `VISION/08-Architecture/Deployment-Model.md`).

## Testing

- `mimir-core/src/auth.rs` unit tests cover token creation (`0600` permissions, hex content, idempotence, atomic publish, canonical-token return on a creation race, empty-file rejection, parent-directory creation) and constant-time verification.
- `mimir-server/tests/auth_tests.rs` covers missing/wrong/malformed tokens on read and write routes, the `WWW-Authenticate` challenge, correct-token acceptance, and the unauthenticated `/health` exception.
- `mimir/tests/e2e.rs` proves the end-to-end contract: an unauthenticated request to `/kb/query` gets `401` while `mimir status` keeps working because the CLI auto-discovers the token.
- The shared server-test fixture (`mimir-server/tests/common/mod.rs`) builds `AppState` with `TEST_TOKEN` and provides `authed_request()` so route tests present the token without repeating the header.

## Migration Notes

- Existing installs: the token is created lazily by the daemon or CLI; no manual step is required.
- Old CLI + new daemon: an old CLI does not send the token and receives `401`; upgrade the CLI binary together with the daemon.
- New CLI + old daemon: the extra `Authorization` header is ignored by an old daemon, so a newer CLI still works against an older daemon.
