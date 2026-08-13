# E2E Testing Architecture

## Overview

The end-to-end test in `mimir/tests/e2e.rs` validates the full request lifecycle:

1. A temporary directory is created.
2. An isolated `config.toml` is written pointing the server and memory to the temp dir.
3. The server is started **in-process** via `mimir_server::start_server_with_llm_and_listener`, using a pre-bound [`TcpListener`] on an ephemeral port and a [`MockLlmClient`] injected as the LLM backend.
4. The test polls `/status` until the daemon reports healthy.
5. The real `mimir` CLI binary is spawned (`mimir ask --no-stream hello`) with `MIMIR_BASE_URL` set.
6. stdout is asserted to contain the mock response.
7. `mimir stop` is sent via the CLI binary and the in-process server task is asserted to exit within 5 seconds.

## Why In-Process Server + OS Process CLI?

The daemon runs in-process so tests can inject a [`MockLlmClient`] directly via the public `start_server_with_llm_and_listener` API. This avoids compiling test-only code into the production binary.

The CLI commands (`mimir ask`, `mimir stop`) are spawned as real OS processes via `env!("CARGO_BIN_EXE_mimir")` so the actual argument parsing, env handling, and HTTP client paths are exercised.

## Mock LLM Injection

The E2E test creates a [`MockLlmClient`] with deterministic responses and passes it to `start_server_with_llm_and_listener`. The production `mimir` binary never sees mock code; the mock backend lives only in the test process.

## Environment Variables Used

| Variable | Purpose |
|----------|---------|
| `MIMIR_BASE_URL` | Overrides `DEFAULT_BASE_URL` in all CLI commands. Cached with `LazyLock`. |
| `MIMIR_MEMORY_PATH` | Overrides the memory file path at runtime. |
| `HOME` | Isolates platform-specific home directories (critical on macOS where `dirs` ignores XDG). |
| `XDG_CONFIG_HOME` | Isolates config for spawned CLI processes. |
| `XDG_DATA_HOME` | Isolates data (DB, memory) for spawned CLI processes. |

## Extending the Harness

To add a new E2E scenario (e.g. streaming chat, memory viewing):

1. Reuse the same temp-dir + isolated-config pattern.
2. Start the server in-process with a pre-bound listener and injected mock backend.
3. Set `MIMIR_BASE_URL` on every CLI invocation.
4. Assert on captured stdout/stderr.

## Connector E2E (Phase 3 T1 / issue #206)

`mimir/tests/connector_e2e.rs` extends the same pattern to the connector framework. The shared `TestDaemon` fixture (`mimir/tests/common/mod.rs`) starts the daemon with the `mock-connector` feature so the `gmail/test` mock backend is registered, and `run_cli_json` runs a CLI subcommand and parses its JSON stdout (asserting success first).

The fact-ingestion tests configure the mock's `facts` knob via `mimir connector add --config-json`, then drive add → auth → resume → sync and verify the knowledge graph through the real HTTP surface:

- `mimir kb query <entity> --json` — fact presence, predicate/object, and confidence (Gmail reliability score = 0.85).
- `mimir kb show <fact_id> --json` — provenance: a `Connector` source with the instance id and `raw_reference`.
- `mimir connector status --json` — `sync_cursor` persistence and the derived per-instance `item_count`.

Corroboration is exercised end-to-end: a second instance emitting the same claim merges into the existing fact row (entity resolution), adds an independent source, and boosts confidence to 0.90; a plain re-sync of the first instance is asserted to be a re-statement no-op (no extra source, no further boost). The supervisor-level round trip (polling + push modes) lives in `mimir-connectors/tests/mock_ingestion_e2e.rs`.

## Mock OAuth server + PKCE E2E (Phase 3 T2 / issue #207)

The interactive PKCE login (A4 / #205) is exercised end-to-end against an in-process mock OAuth server instead of a real provider:

- **`mimir-connectors/src/mock_oauth.rs`** (feature `test-mock-oauth`, off by default) is a two-listener loopback server sharing one state object: an HTTPS `GET /authorize` (the flow's `auth_uri` gate requires HTTPS; the self-signed certificate is generated per test run with `rcgen`) and an HTTP `POST /token` (loopback HTTP is the shared token-endpoint gate's local trust boundary). The authorize endpoint records the request, issues a one-time code, and redirects to the client's `redirect_uri` with the CSRF `state` echoed; the token endpoint validates the PKCE S256 `code_verifier` against the challenge captured at authorize time, enforces one-time code use, and issues an OAuth token bundle. Both endpoints record every request for assertions.
- **`mimir-connectors/tests/oauth_pkce_e2e.rs`** drives `run_pkce_flow` against the mock server with a fake-browser opener that GETs the authorize URL (accepting the test certificate) and follows the redirect into the loopback callback — the full authorize → redirect → callback → exchange round trip. Mock-correctness tests cover the state echo, one-time code replay rejection, wrong-verifier rejection, unknown grant types, non-S256 challenge-method rejection, and CR/LF rejection in the redirect URI and state.
- **`mimir/tests/connector_oauth_e2e.rs`** drives the real `mimir connector add` CLI against the real daemon with `auth.kind=oauth` config: the CLI's `webbrowser` call is redirected to a `$BROWSER` fake-browser script (`curl -k -L`) that follows the HTTPS authorize redirect, and the exchanged tokens land in the daemon's secret store (`auth_state=authenticated`), after which the instance can be resumed and synced.
- **Shared fake-browser test doubles (`mimir-connectors/src/test_utils.rs`, feature `test-utils`, #290).** The PKCE flow's unit tests (`oauth::pkce`) and the CLI connector tests (`mimir/src/connector/tests.rs`) used to each carry a private `self_callback_opener` copy; the shared module now owns `parse_authorize_url` (redirect URI + CSRF state), `callback_url` (code + state echo), and `self_callback_opener(code)` once, and the inline variant openers (wrong-state, favicon-probe) build on the same parsing. The e2e openers above are intentionally not collapsed into it — they must accept the mock's self-signed certificate and follow the redirect.

The `test-mock-oauth` feature is enabled for the workspace test run through the `mimir` binary's dev-dependencies, so `cargo test --workspace` executes these tests; a standalone `cargo test -p mimir-connectors` needs `--features test-mock-oauth`.

## Rate-limit/backoff + supervisor edge-case tests (Phase 3 T2 / issue #207)

- **`mimir-connectors/tests/rate_limit_http.rs`** verifies the F12 primitives over real HTTP: a wiremock endpoint returning 429 with `Retry-After` (and 503) is retried by `retry_with_backoff` with the server hint driving the wait, and a `RateLimiter` with `daily_quota=Some(N)` stops issuing HTTP calls once the quota is spent (the exhaustion surfaces as a non-retryable `QuotaExhausted` and the wiremock `expect` proves no further request).
- **`mimir-connectors/tests/supervisor_lifecycle_tests.rs`** covers the F8 edge cases: startup restore, graceful-shutdown cursor persistence, circuit breaker (both ordinary failures and repeated panics), and panic recovery.
