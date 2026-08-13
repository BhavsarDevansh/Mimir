# Changelog

## [0.101.0] — 2026-08-13

### Autonomous development loop

- **`scripts/autonomous-loop.sh` (new).** Deterministic bash orchestrator that runs on a configurable cadence (default 2h; `MIMIR_AUTONOMOUS_INTERVAL=1800` for a 30-minute cadence) and drives the whole GitHub lifecycle: on `main` it picks the next unblocked issue (blocked and unanswered `help-wanted` issues are skipped, active phase first), implements it via `codex exec` + the `gh-issue-tdd` skill, and publishes a DRAFT PR; on a feature branch it self-reviews the draft against `main` (fixing every finding), marks it ready, addresses unresolved review threads via the `gh-review-commit` skill, and merges once clean — explicitly logging when CodeRabbit skipped its review. Control flow stays in deterministic bash; only open-ended engineering is delegated to the agent.
- **Review and merge gates.** Review-thread pagination omits the initial GraphQL cursor and then follows returned cursors, and auto-merge now requires GitHub's `CLEAN` merge state rather than treating behind or unstable PRs as mergeable.
- **Issue hygiene built into the loop.** The implementation prompt fetches the full issue (all fields plus every comment), validates the spec against the current codebase (updating stale issue bodies with current context), posts exact human requirements with the `help-wanted` label and the `<!-- mimir-autonomous-question:N -->` marker when blocked, files new issues for out-of-scope problems using only existing labels, and keeps `README.md`, `docs/wiki/what-works-now.md` and `AGENTS.md` accurate.
- **Optional systemd user timer (`scripts/systemd/`).** `mimir-autonomous.service` + `.timer` run one iteration every 2h with `Persistent=true`, so missed runs are caught up on boot.
- **Docs:** `docs/autonomous-loop.md`, `docs/wiki/autonomous-loop.md`, `README.md` (Autonomous Development Loop section) and `AGENTS.md` (Issue Hygiene & Autonomous Loop section) added/updated, with prose on the single-line markdown standard.
- Version bumped 0.100.0 → 0.101.0 (minor — backwards-compatible new maintenance subsystem).

## [0.100.0] — 2026-08-12

### DRY: shared OAuth fake-browser test doubles (issue #290)

- **`mimir-connectors::test_utils` (feature `test-utils`, off by default).** The `self_callback_opener` fake-browser helper was duplicated verbatim across `mimir-connectors/src/oauth/pkce.rs` (unit tests) and `mimir/src/connector/tests.rs` (CLI add/auth e2e tests), introduced by A4 / #205. The shared module now owns the drift-prone pieces once: `parse_authorize_url` (redirect URI + CSRF state extraction), `callback_url` (code + state echo), and `self_callback_opener(code)` (drives the loopback callback). The crate's own unit tests compile the module via `cfg(test)`; downstream crates opt in with the feature (the `mimir` binary's dev-dependencies enable it). No new dependencies — the helpers only use `reqwest` and `tokio`, both already unconditional.
- **Both test suites refactored onto the shared helper.** `oauth::pkce` tests dropped their local copy and the two inline variant openers (wrong-state, favicon-probe) now build on `parse_authorize_url` / `callback_url`; `mimir/src/connector/tests.rs` imports `self_callback_opener("auth-code")` at both call sites. The e2e openers (`browser_opener` in `mimir-connectors/tests/oauth_pkce_e2e.rs`, the `$BROWSER` curl script in `mimir/tests/connector_oauth_e2e.rs`) stay separate because they must accept the mock's self-signed certificate and follow the redirect — different mechanics, not the same drift-prone code.
- **Docs:** `docs/oauth-client.md`, `docs/e2e-testing.md`, `docs/wiki/Testing-and-Benchmarks.md`, `docs/wiki/what-works-now.md` (#290 row removed), `README.md`, `AGENTS.md` (test-only feature convention), and `Mimir-Implementation-Context.md` updated.
- **Review fixes (PR #297):** `callback_url` now appends `code`/`state` as percent-encoded query pairs via a URL query builder (reserved characters such as `+`, `#`, `%`, `&` are no longer mangled, and an existing query on the redirect URI is preserved with the correct separator), the loopback test bounds every accept/read wait with a 5-second deadline so a broken opener fails fast instead of hanging, and the README documentation references are clickable links.
- Version bumped 0.99.0 → 0.100.0 (minor — backwards-compatible refactor of test infrastructure).

## [0.99.0] — 2026-08-12

### Phase 3 T2 — mock OAuth server + PKCE/rate-limit/supervisor E2E tests (issue #207)

- **In-process mock OAuth 2.0 authorization server.** `mimir-connectors::mock_oauth` (feature `test-mock-oauth`, off by default) serves the two endpoints the interactive PKCE flow needs without a real provider: an HTTPS `GET /authorize` (self-signed `rcgen` certificate generated per test run; the flow's HTTPS-only `auth_uri` gate is honoured) issues a one-time code and redirects to the loopback callback with the CSRF `state` echoed, and an HTTP `POST /token` validates the PKCE S256 `code_verifier` against the challenge captured at authorize time, enforces one-time code use, and issues an OAuth token bundle. Both endpoints record every request for assertions.
- **PKCE flow E2E against the mock server.** `mimir-connectors/tests/oauth_pkce_e2e.rs` drives `run_pkce_flow` through the full authorize → redirect → loopback callback → code-exchange round trip with a fake-browser opener, and asserts the exchanged `SecretBundle` contents plus the exact authorize/token request shapes. Mock-correctness tests cover the state echo, one-time code replay rejection, wrong-verifier rejection, and unknown grant types.
- **Daemon-level OAuth E2E.** `mimir/tests/connector_oauth_e2e.rs` drives the real `mimir connector add` CLI against the real daemon with `auth.kind=oauth` config: the CLI's `webbrowser` call is redirected to a `$BROWSER` fake-browser script (`curl -k -L`) that follows the HTTPS authorize redirect, and the exchanged tokens land in the daemon's secret store (`auth_state=authenticated`), after which the instance can be resumed and synced.
- **Rate-limit/backoff tests over real HTTP.** `mimir-connectors/tests/rate_limit_http.rs` verifies the F12 primitives against a wiremock endpoint: 429 with `Retry-After` (and 503) are retried by `retry_with_backoff` with the server hint driving the wait, and a `RateLimiter` with `daily_quota=Some(N)` stops issuing HTTP calls once the quota is spent (the exhaustion surfaces as a non-retryable `QuotaExhausted` and the wiremock `expect` proves no further request).
- **Supervisor edge-case tests.** `mimir-connectors/tests/supervisor_lifecycle_tests.rs` now covers the F8 edge cases: startup restore, graceful-shutdown cursor persistence, circuit breaker (both ordinary failures and repeated panics), and panic recovery.
- **Test harness DRY.** `TestDaemon` gains `run_cli_with_env` (extra env vars, e.g. `BROWSER` for the OAuth E2E) alongside the existing `run_cli` / `run_cli_json` helpers.
- **Docs:** `docs/e2e-testing.md` (T2 sections), `docs/oauth-client.md` (feature gating + testing), `docs/wiki/what-works-now.md` (PKCE + E2E harness rows → ✅ Works, #207 removed from the Connectors work items), and `docs/wiki/Testing-and-Benchmarks.md` updated.
- Version bumped 0.98.0 → 0.99.0 (minor — backwards-compatible new test deliverable, matching the 0.97.2 → 0.98.0 precedent for T1 / #206).

## [0.98.0] — 2026-08-12

### Phase 3 T1 — mock connector sync→normalize→insert→query E2E harness (issue #206)

- **Daemon-level fact-ingestion E2E tests.** `mimir/tests/connector_e2e.rs` now configures the `gmail/test` mock connector's `facts` knob and drives the full pipeline through the real CLI + in-process daemon: `connector add --config-json` → auth → resume → sync → `kb query` / `kb show`. The tests assert facts land with `source_type=Connector`, provenance tied to the connector instance (`connector_instance_id` + `raw_reference`), confidence from the connector reliability score (Gmail = 0.85), sync-cursor persistence, and the derived per-instance `item_count`.
- **Corroboration path exercised end-to-end.** A second connector instance corroborating the same claim merges into the existing fact row (entity resolution), adds an independent source, and boosts confidence to 0.90 (+0.05, capped at 0.95); a plain re-sync of the same instance is asserted to be a re-statement no-op (no extra source, no further boost).
- **Supervisor-level confidence assertion.** `mimir-connectors/tests/mock_ingestion_e2e.rs` now asserts the exact Gmail reliability score (0.85) instead of a loose `> 0.0` check.
- **Test harness DRY.** `TestDaemon` gains a `run_cli_json` helper (asserts success + parses JSON stdout); the existing lifecycle e2e test was refactored onto it.
- **Docs:** `docs/e2e-testing.md` (connector E2E section), `docs/mock-connector.md` (T1 status), `docs/connector-management.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md` (E2E harness row → ✅ Works), `docs/wiki/Testing-and-Benchmarks.md`, `README.md`, and `Mimir-Implementation-Context.md` updated.
- Version bumped 0.97.2 → 0.98.0 (minor — backwards-compatible new test harness deliverable, matching the 0.94 → 0.95 precedent for the mock-connector daemon feature).

## [0.97.2] — 2026-08-12

### Review fixes — PKCE flow security hardening and OAuth config back-compat (PR #291)

- **HTTPS-only authorization endpoint.** `run_pkce_flow` now rejects any `auth_uri` whose scheme is not `https` before the browser is opened (RFC 8252 §7.5) — the authorization endpoint carries the user's credentials, so plain HTTP is never allowed there even though the token endpoint gate permits loopback HTTP.
- **Fixed callback error page.** A callback that aborts the flow (provider `error` param, missing/incorrect `state`) now responds with a fixed HTML page instead of echoing the provider-controlled `error` value into the browser (XSS on the loopback origin); the diagnostic stays in the process error only.
- **Stored OAuth configs without `auth_uri` load again.** `CalendarAuthMethod::OAuth` and `EmailAuthMethod::OAuth` persist `auth_uri` as optional (`#[serde(default)]`), so records created before the field existed (pre-0.97.0) deserialize instead of failing at startup; new configs still require `auth_uri` via the JSON schema, and the interactive PKCE flow still fails with a clear message when it is absent.
- **`oauth::pkce` is now a public module** (`mimir_connectors::oauth::pkce`), alongside the existing root re-exports.
- **OAuth progress output moved to stderr.** The printed authorize URL and the "Starting OAuth login" message go to stderr, so `mimir connector add/auth --json` stdout stays valid JSON for scripts.
- **Docs:** `docs/wiki/cli-commands.md` documents that `auth.scopes` must be supplied via `--config-json` (the `key=value` parser drops JSON arrays, #289); `docs/wiki/connectors.md` documents the required `mimir connector resume <slug>` step after `add`/re-auth; the VISION onboarding example uses the documented `mimir connector add gmail --backend <b>` syntax; `docs/oauth-client.md` security properties updated.
- **Tests:** non-https `auth_uri` rejected without opening the browser, callback error page does not echo provider input, and stored-record deserialization without `auth_uri` for both Calendar and Email.
- Version bumped 0.97.1 → 0.97.2 (patch — bug fixes, no API breakage).

## [0.97.1] — 2026-08-12

### Review fixes — PKCE loopback flow robustness and timeout UX (PR #291)

- **Per-connection read deadline on the loopback callback listener.** A connection that sends nothing (or a partial request) is now dropped after a 10-second deadline instead of stalling the whole flow until the 5-minute overall timeout — a stalled or hostile local process can no longer waste the user's authorization. The dropped connection is ignored and the flow keeps waiting for the real callback.
- **Timeout error no longer points at a dead login.** When the flow times out, the error now states that the flow aborted and the command must be re-run, instead of telling the user to complete a login whose loopback listener is already closed.
- **Tests:** stalled-connection read timeout, stalled connection followed by a real callback, and the timeout message wording.
- **Docs:** `docs/oauth-client.md` security properties and `docs/wiki/cli-commands.md` updated with the per-connection deadline and timeout behaviour.
- Version bumped 0.97.0 → 0.97.1 (patch — bug fixes, no API change).

## [0.97.0] — 2026-08-11

### Phase 3 A4 — interactive OAuth PKCE loopback flow (issue #205)

- **`mimir-connectors::oauth::pkce` — the interactive PKCE authorization-code flow.** `run_pkce_flow` binds an ephemeral loopback listener on `127.0.0.1:0`, builds the provider's authorize URL with an S256 PKCE challenge + CSRF state, receives the redirect (8 KiB read cap, state validated, favicon probes ignored), exchanges the code via the shared `OAuthHttpClient` (HTTPS/loopback token-endpoint gate + secret-hygiene error mapping), and returns the `SecretBundle::OAuth` for the caller to persist. The daemon never runs a transient HTTP server. Public surface: `PkceFlowConfig`, `run_pkce_flow`, `DEFAULT_FLOW_TIMEOUT` (gated by the `oauth` feature, which now also gates `url`).
- **`mimir connector add` / `auth` run the flow for `auth.kind=oauth` configs.** `add` acquires the credential *before* registering the instance (a canceled prompt or aborted OAuth flow exits with nothing created) and POSTs the exchanged bundle to the daemon's token-ingest route so the instance becomes `authenticated`. `auth` re-runs the flow for expired credentials, taking the OAuth client config from re-supplied `key=value` / `--config-json` args (the daemon does not expose the stored config on the wire). The authorize URL is printed before the browser is opened (`webbrowser` 1.2.4), so headless/SSH sessions can complete the login manually; a browser-open failure is non-fatal.
- **Breaking config change: `auth_uri` is now required on OAuth auth methods.** `CalendarAuthMethod::OAuth` and `EmailAuthMethod::OAuth` gained a required `auth_uri` field (the provider's authorization endpoint), reflected in the JSON schemas and config docs. Existing OAuth configs must add `auth.auth_uri` before the flow can run.
- **Tests:** 12 new `run_pkce_flow` / `parse_callback` tests (happy path, state mismatch, timeout, non-HTTPS token endpoint, invalid auth URI, favicon probe, provider-error param, percent-decoding, secret-hygiene error mapping, refresh-token retention, expiry clamping) plus CLI e2e tests for the add/auth PKCE paths against a wiremock token endpoint, config extraction, and ingest conversion.
- **Docs:** `docs/oauth-client.md` documents the flow and its security properties; connector-management, CLI, email/calendar connector, wiki, README, `Mimir-Implementation-Context.md`, and the VISION Phase 3 plan updated; `VISION/03-Connectors/User-Experience.md` onboarding example updated from paste-a-code to the loopback flow.
- Version bumped 0.96.2 → 0.97.0 (minor — new feature; breaking config change acceptable per project policy).

## [0.96.2] — 2026-08-11

### Docs — what-works-now.md rewritten as a feature-level roadmap

- **`docs/wiki/what-works-now.md` rewritten.** The changelog block in the header is removed (release history lives in this file), the Phase 3 roadmap wall-of-text is condensed to a summary, and the feature reference is expanded into an honest per-feature status guide (✅ works / 🟡 partial / ❌ not implemented) with the pending work for each feature linked to its GitHub issue. New sections cover Events & Reminders, Connectors, Background Jobs, the LLM client/worker pool, and the Librarian/Retrieval agents; the API endpoint table now matches the real route set; the Known Limitations table drops the closed #71/#45 entries and lists the current open issues.
- **Nine new GitHub issues created from the audit** (#279–#287): session compaction, chat session persistence, HTTP API authentication, LLM semantic entity dedup, email iMIP CANCEL lifecycle, memory pinning, macOS launchd, config hot-reload propagation, and a small code-quality cleanup.
- Version bumped 0.96.1 → 0.96.2 (patch — documentation only; no code or behaviour change).

## [0.96.1] — 2026-08-11

### Review fixes — oauth2/reqwest reconciliation (PR #278)

- **OAuth HTTP client built only for OAuth auth methods.** `CalendarConnector` and `EmailConnector` now construct the hardened `OAuthHttpClient` only when the config uses `CalendarAuthMethod::OAuth` / `EmailAuthMethod::OAuth` (stored as `Option`); an app-password connector no longer allocates a second reqwest connection pool and can no longer fail startup on an OAuth client build error.
- **Token-response body capped at 64 KiB.** The `OAuthHttpClient` adapter streams the token-endpoint response with an explicit bound instead of buffering it whole, so a compromised or misconfigured endpoint cannot force a large allocation on the refresh path of a long-running daemon.
- **Hostile `expires_in` clamped to 90 days.** `expires_at_from_now` no longer saturates at chrono's `MAX_UTC` (which made `needs_refresh` permanently false and reused a dead access token forever); absurd provider values now clamp to a plausible 90-day lifetime.
- **`OAuthHttpClient::from_client` narrowed to `pub(crate)`** (test-only escape hatch around the redirect hardening), and the crate-level docs no longer link to the feature-gated `oauth` module from the no-feature doc build.
- **Tests:** unknown-expiry reuse test, oversized-body rejection tests, and the network-cause assertion no longer depends on reqwest's internal `"builder error"` display string.
- **Docs:** the OAuth error contract is now stated consistently across the connector docs (provider response errors expose only parsed `error`/`error_description`; network failures include the underlying reqwest error detail; raw response bodies are never surfaced), the A4 CLI PKCE flow is marked planned/reserved in the framework table, and the Email OAuth credential model documents the optional `client_secret` (sent in the token-refresh request body when configured, never stored or logged).

## [0.96.0] — 2026-08-11

### OAuth2 / reqwest reconciliation (issue #240)

- **`oauth2` 5.0.0 joins the tree with `default-features = false` and a custom HTTP adapter.** The Phase 3 deps ledger mandates `oauth2` for Calendar/Email refresh and the A4 PKCE login, but the crate's optional `reqwest` feature pins reqwest 0.12, which would duplicate the workspace's reqwest 0.13 HTTP/TLS stack. The reconciliation: a new `OAuthHttpClient` newtype (`mimir-connectors::oauth`) implements the crate's `AsyncHttpClient` trait over the workspace reqwest 0.13 client — `HttpRequest`/`HttpResponse` are plain `http` 1.x types shared by both reqwest lines, so the adapter is the same pattern as oauth2's own `reqwest_client.rs` with no reqwest 0.12 in the tree. Gated by a new `oauth` feature (enabled by `calendar` and `gmail`; the CLI PKCE flow A4 / #205 builds on it).
- **Refresh migrated from the hand-rolled POST to the vetted grant.** The shared `oauth::refresh_token` helper now drives `oauth2`'s `exchange_refresh_token` (form-body client credentials, refresh-token rotation retained, `expires_in` → `expires_at` unchanged), and the duplicated expiry-check/refresh/persist logic in the Calendar and Email `resolve_auth` arms is DRY-extracted into `oauth::resolve_access_token` — one OAuth refresh path, ~300 lines of hand-rolled protocol code deleted.
- **Security hardening preserved and extended.** The HTTPS/loopback token-endpoint gate and the secret-hygiene error mapping (parsed `error`/`error_description` only, truncated at 256 bytes, raw response body never surfaced) carry over unchanged; the OAuth client is additionally built with `redirect::Policy::none()` so a credential POST can never be bounced to another host (the pre-#240 refresh followed redirects by default). Client credentials stay in the form body (`AuthType::RequestBody`), matching prior behaviour.
- **Dead code removed:** the Email connector's now-unused shared `reqwest::Client` field/param is gone (`from_config_with_http` → `from_config_with_deps`), and reqwest's `form` feature is dropped from `mimir-connectors` (the `oauth2` crate builds its own form-encoded body).
- **Tests:** 27 new/updated tests — wiremock refresh round-trips (request shape, token parsing, rotation, scope joining), secret-hygiene + truncation, redirect non-following (attacker host sees zero requests), HTTPS/lookalike-loopback gates, skew-window refresh, and connector-level expired-token refresh for both Calendar and Email. Full workspace suite green; clippy/fmt clean. New deps: `oauth2` 5.0.0 (+ `http` 1.x declared directly); `rand 0.8` enters the tree as a third rand line (oauth2's PKCE verifier generation; 0.9/0.10 already present) — recorded in the deps ledger.
- **Review fixes:** network-level refresh failures now surface the underlying reqwest cause — oauth2's `HttpClientError::Reqwest` display is the constant `client error` (the inner error is not part of the format string), so the adapter formats the inner error (DNS / timeout / TLS / connection) into `ConnectorError::Network` — and a hostile `expires_in` saturates at chrono's `MAX_UTC` instead of panicking the refresh path (`DateTime + TimeDelta` overflows on values beyond year 262143; `Duration::MAX` is far beyond that range).
- **Docs:** deps ledger (`VISION/09-Roadmap/Phase-3-Plan.md` §4) records the chosen path; new technical doc `docs/oauth-client.md`; `docs/calendar-connector.md`, `docs/email-connector.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` updated. `AGENTS.md` needs no change (no rule or ledger reference became stale). Version bumped 0.95.1 → 0.96.0 (minor — backwards-compatible new dependency + internal refactor). The `EmailConnector::from_config_with_http` → `from_config_with_deps` rename is a **public API change** (`EmailConnector` is re-exported from `mimir-connectors`): the injected `reqwest::Client` parameter is removed (the connector now builds its own OAuth HTTP client from config) and the final parameter is the optional LLM backend. Migrate callers to `from_config_with_deps(config, secret_store, user_identity, cursor, llm_backend)`.

## [0.95.1] — 2026-08-11

### Review fixes — `mimir connector` CLI (PR #272)

- **CLI correctness:** `connector add` now rejects `--password` together with `--token` (clap `conflicts_with`, matching `connector auth`), and a failed credential ingest after a successful `add` prints the server error plus the recovery hint `mimir connector auth <slug>` so an unauthenticated instance is never a dead end.
- **Config parsing:** `key="value"` pairs in `connector add` keep their string type — double-quoted scalars no longer coerce (`account="0755"` stays `"0755"`, `version="1.0"` stays `"1.0"`) — while unquoted values keep the existing boolean/number coercion.
- **Test hardening:** wiremock mounts in the connector unit tests now assert exact hit counts (`.expect(1)`), and the binary-level `mimir connector` tests isolate `HOME`, `XDG_CONFIG_HOME`, and `XDG_DATA_HOME` in a `tempfile` directory so they never touch the host home directory.
- **Docs:** release docs now match the CLI — `auth` added to the subcommand inventories, `--json` qualified (every subcommand except `remove`), `remove`/`forget` documented as alternative teardown operations (not a sequence), Quick Start uses one polling connector (`gmail`) with the required `resume` activation, connector status pages say library + daemon/CLI integration are implemented, and the Phase 3 roadmap summary reflects that only A4 (#205, OAuth PKCE) remains.

## [0.95.0] — 2026-08-11

### CLI — `mimir connector` subcommands (Phase 3 A3 / #204)

- **Ten connector subcommands plumb the A1/A2 daemon routes** through `mimir-client` in a new `mimir/src/connector/` module (clap definitions in `mimir/src/cli.rs`, dispatch in `main.rs`, one handler module per concern — `add`/`auth`/`query`/`lifecycle`/`sync`/`actions`). `add <type> --backend <b> [key=value...]` registers an instance in `Setup` (dotted config keys nest, `--config-json` provides a base object, scalars parse as booleans/numbers/strings); `list` and `status [slug]` render tabled/coloured overviews; `sync <slug> [--full | --since <dur>]` triggers a manual cycle (human durations `30s`/`5m`/`12h`/`7d` or bare seconds, `--full`/`--since` mutually exclusive); `pause`/`resume` control the runner; `remove` detaches provenance (facts survive) while `forget` cascade-trashes the connector's facts (recoverable 30 days), credentials, and row; `act <slug> <kind> [payload | --json-file]` dispatches write-backs (Calendar `create_event`/`update_event`/`delete_event`) and echoes the `ActionResult`. Every subcommand except `remove` supports `--json` output consistent with `kb`.
- **Non-OAuth credential ingest:** when the merged config declares `auth.kind=app_password`/`api_token`, `add` prompts for the secret via `inquire` *before* registering the instance — a canceled prompt aborts with nothing created — and ingests it through `POST /connectors/{id}/tokens` (`--password`/`--token` flags make it non-interactive; a non-interactive run without a flag registers the instance unauthenticated and warns). The new `auth <slug> [--password | --token]` subcommand re-ingests credentials on an existing instance (the recovery path for that warning and for expired credentials, without `remove` + re-`add`); the credential kind comes from the flag or an interactive selection, since the daemon does not expose the stored config on the wire. OAuth configs never prompt — the PKCE flow remains A4 (#205). Destructive subcommands confirm via `inquire` with a `--yes` skip, and `sync` surfaces the `CONNECTOR_NOT_RUNNING` 409 with an activation hint (`mimir connector resume <slug>`).
- **Slug resolution + error rendering:** slug-based subcommands resolve slugs client-side against `GET /connectors` (no by-slug route), and daemon `ApiError` JSON bodies are unwrapped so users see the human detail instead of raw JSON.
- **Shared CLI helpers (DRY):** `exit_with_error`, `make_client`, and `print_json` move to `mimir/src/cli_util.rs`, reused by both the `kb` and `connector` command groups (previously duplicated in `kb/mod.rs` and `commands.rs`).
- **`mock-connector` daemon feature:** `mimir-server` gains a `mock-connector` feature (default off) that registers the mock factory in the daemon registry (previously `cfg(test)`-only), enabling a real CLI e2e cycle — `mimir/tests/connector_e2e.rs` runs add → status → auth → resume → sync → pause → resume → remove on one instance, then add → forget on a second (remove deletes the row, so forget always targets a fresh instance) against an in-process daemon with the mock backend (the `auth` step asserts the credential ingest flips `auth_state` to `authenticated`); `mimir/tests/common/mod.rs` extracts the shared `TestDaemon` fixture from `e2e.rs` (DRY), and `mimir/tests/connector_cli_tests.rs` covers the full clap → daemon-guard → HTTP path against wiremock. 30 new tests total (25 unit/wiremock, 4 binary-level, 1 e2e).
- **Docs:** `docs/cli.md` (new `mimir connector` reference + corrected "direct library linkage" overview), `docs/connector-management.md` (A3 section), `docs/wiki/cli-commands.md` (connector section), `docs/wiki/connectors.md` (CLI usage), `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` updated; stale `A4 / #206` references corrected to `#205` across connector docs. Version bumped 0.94.0 → 0.95.0 (minor — backwards-compatible new feature).

## [0.94.0] — 2026-08-10

### Module-split refactor

- **Oversized files broken into single-concern modules:** every file that mixed multiple responsibilities is now a directory of small modules with the public API re-exported unchanged from the root module. In `mimir-connectors`: `supervisor` (`config`/`error`/`trigger`/`runner`/`control`/`cycle`), `calendar` (`construct`/`credentials`/`sync`/`trait_impl`/`payload` + `caldav/`), `email` (`config`/`factory`/`imap`/`connector/`/`jsonld/`/`llm/`), `rate_limit/`, `geocoder/`, `ical/`, `mock/`, `photos/`, and `secrets` (`error`/`bundle`/`store`/`file`/`memory`). In `mimir-core`: `config/`, `context/`, `job_queue/`, `llm/client/`, and `llm/pool/`. In `mimir-knowledge`: `extract/`, `normalize/`, `forget/`, `queries/entity/`, `queries/fact/`, `queries/memory/`, `queries/preference/`, and `optimization/`. In `mimir-server`: `routes/kb/`, `state/`, plus `app.rs`/`server.rs`/`shutdown.rs` extracted from `lib.rs`. In `mimir-client`: `kb/`. In `mimir-api-types`: `chat.rs`/`connectors.rs`/`kb.rs`/`kb_maintenance.rs`.
- **Integration suites split per feature:** `fact_management_test.rs` (2627 lines) → eight fact-focused suites; `integration_tests.rs`, `extraction_test.rs`, `calendar_connector.rs`, `supervisor_lifecycle.rs`, `kb_tests.rs`, and `chat_tests.rs` each split into per-concern files with shared fixtures in `tests/common/`.
- **Documentation refresh:** `docs/refactoring-module-split.md` and `docs/wiki/module-split.md` document the new module map and rationale; subsystem docs (`workspace`, `chat-server`, `config-system`, `llm-client`, `knowledge-graph-schema`, `connectors-framework`, and others) now reference the new module locations.
- **Quality pass:** zero-warning `cargo clippy --workspace --all-targets` (fixed orphaned doc-comment blocks, `module_inception` via `supervisor/runner.rs`, `too_many_arguments` allowances, and a `doc_lazy_continuation`); `cargo fmt --all` clean; full workspace suite 1476 passed / 0 failed. Version bumped 0.93.0 → 0.94.0 (minor — refactor, no public API or behaviour changes).

## [0.93.0] — 2026-08-10

### Connectors — PR #268 review feedback

- **Forget cascade invokes `Connector::forget()` (contract fix):** the daemon's forget route now calls a new `ConnectorSupervisor::forget(id)` which stops the runner and invokes the connector's local `forget()` cleanup on the live instance (or a freshly re-instantiated one when no runner is alive), honouring the `Connector` trait contract that connector-local cleanup is the supervisor's job. The cascade is serialised per connector via a new `ConnectorSupervisor::lifecycle_lock(id)` (also acquired by `start`/`resume`), marks the instance `Paused` first so an aborted cascade leaves a state a retry can reason about, and deletes the secret *before* the irreversible fact trash so a credential-deletion failure aborts with nothing destroyed.
- **Credential-ingest and forget routes are loopback-only:** `POST /connectors/{id}/tokens` and `/forget` now carry the `require_loopback` layer like the other sensitive/destructive endpoints; a non-loopback caller gets `403` before any mutation (new route test).
- **OAuth failure details masked in `401` responses:** `ConnectorError::Authentication` now returns the fixed `"authentication failed"` body (full detail stays in the server log) so a provider-echoed token-endpoint response can never leak credentials back to the caller (new unit test).
- **`IngestTokenRequest` gets a redacting `Debug` impl:** the derived `Debug` printed `access_token` / `refresh_token` / `token` / `password` verbatim; the manual impl prints `<redacted>` while keeping variant tags and optional-field presence visible (new unit test).
- **`spawn_into` now enforces its documented invariant:** it stops any existing runner for the row before inserting the new handle, so a re-spawn can never detach a live task; `start`'s redundant pre-stop is removed (the lifecycle lock now serialises against the forget cascade).
- **`ActError` gains `From<SupervisorError>`:** the manual `map_err` at the dispatch site is replaced with `?`, so a new `SupervisorError` variant is caught by the compiler instead of silently falling through.
- **Shared batch-trash helper in `mimir-knowledge`:** the third verbatim copy of the chunked trash loop is extracted into `forget::trash_ids_in_batches` (with a `TRASH_BATCH_SIZE` const), used by `forget_facts`, `forget_all`, and `forget_facts_for_connector`.
- **Test hardening:** fixed 50 ms sleeps in supervisor/server tests are replaced with polling helpers (`wait_for_status` / `wait_for_running` / `wait_for_runner_exit`) so loaded CI runners cannot flake; new tests cover multi-source fact trash, `forget` on live / cold / unknown instances, secret deletion in the forget route, and non-loopback rejection.
- **Docs:** `docs/connector-management.md` and `docs/wiki/connectors.md` classify `act` as an action method (not lifecycle), document `start` as an internal supervisor method, refresh the backend status (Calendar/Email extraction + write-back shipped), describe the hardened forget cascade, and remove the remaining A1-era status text; `docs/wiki/what-works-now.md` documents natural runner-exit recovery. Version bumped 0.92.0 → 0.93.0 (patch — review fixes only).
- **Round 2:** `ConnectorSupervisor::forget` now captures the live connector *before* stopping the runner (previously `stop()` removed the handle first, so the live instance was never used and the connector was always re-instantiated — the in-memory state the cleanup must tear down, e.g. the Photos watcher, was dropped instead); the live-path test now asserts the factory is called exactly once. `docs/wiki/connectors.md` fixes the OAuth PKCE issue reference (`#206` → `#205`), and `docs/wiki/what-works-now.md` metadata aligns with 0.93.0 / 2026-08-10.

## [0.92.0] — 2026-08-06

### Connectors — action routes + OAuth token ingest + forget cascade (A2 / #203)

- **Action routes:** six new Axum routes round-trip via `mimir-client` — `POST /connectors/{id}/sync` (manual sync trigger, F9; maps the supervisor's `TriggerOutcome` to a `status`-tagged `SyncConnectorResponse`), `POST /connectors/{id}/pause` and `/resume` (lifecycle control), `POST /connectors/{id}/tokens` (ingest a `SecretBundle` keyed by slug via the shared `SecretStore` and flip `auth_state` to `authenticated`), `POST /connectors/{id}/actions` (dispatch `{ kind, payload }` to the connector's `act()` write-back, e.g. the Calendar connector's `create_event`/`update_event`/`delete_event`), and `POST /connectors/{id}/forget` (cascade-forget: stop the runner → trash every fact sourced from the connector via the existing trash machinery → delete the slug-keyed secret → delete the row). `forget` is recoverable from trash for 30 days, unlike `DELETE /connectors/{id}` (A1) which detaches provenance so facts survive with degraded provenance.
- **Supervisor additions (`mimir-connectors`):** `ConnectorSupervisor::start(id)` re-spawns a single connector (load row → instantiate → stop any existing runner → flip `Active` → spawn), shared via a new private `spawn_into` helper used by both `restore()` and `start()`. `pause(id)`, `resume(id)`, and `act(id, action)` land; each `ConnectorHandle` now retains the live `Arc<dyn Connector>` (cloned from the one moved into the runner) so write-back actions dispatch to the authenticated instance. `act` re-instantiates from the row when no live runner exists — including a connector whose runner exited naturally (auth-expiry, circuit-breaker, panic), whose stale handle is dropped first — so a write-back never runs against expired in-memory credentials after fresh ones are stored via `/tokens`. New `SupervisorError::UnknownConnectorType` variant and a new `ActError` enum.
- **Knowledge-graph facade (`mimir-knowledge`):** `KnowledgeGraph::forget_connector_facts(id, changed_by)` — the `forget` cascade. Soft-deletes (trashes) every fact whose `sources` row carries `connector_instance_id = id` via the shared `forget_fact_tx` trash machinery; `sources` rows are cascade-deleted with their facts (`ON DELETE CASCADE`); no `--yes`/`--confirm-sensitive` gate applies (an explicit admin action). Backed by a new `forget::forget_facts_for_connector` function.
- **Wire types (`mimir-api-types`):** `SyncConnectorRequest`/`SyncConnectorResponse`, `IngestTokenRequest` (a `kind`-tagged mirror of `SecretBundle`, keeping `mimir-api-types` decoupled from `mimir-connectors`), `ConnectorActionRequest`/`ActionResultResponse`, `ForgetConnectorResponse`, with round-trip tests.
- **Server error mapping (`mimir-server`):** a `ConnectorError` → HTTP mapping (`UnsupportedAction`/`Config`/`Parse`→400, `NotAuthenticated`/`Authentication`→401, `Network`→502, `BackendNotFound`→404, `BackendAlreadyRegistered`→409, `Io`/`Other`→500 with detail masked) plus `SupervisorError`, `ActError`, `TriggerError`, and `SecretError` mappings, with unit tests.
- **Mock connector:** `MockConnector` gains an `act_kind` config so the action-dispatch path is testable; unsupported kinds return `UnsupportedAction`.
- **Client (`mimir-client`):** `connector_sync`, `connector_pause`, `connector_resume`, `connector_tokens`, `connector_actions`, `connector_forget` methods.
- **Docs:** updated `docs/connector-management.md`, `docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` to reflect A2 landing. Version bumped 0.91.0 → 0.92.0 (minor — backwards-compatible new feature routes and facade methods).



### Connectors — PR #263 review feedback (round 2)

- **Delete stored credentials with the connector instance (security):** `DELETE /connectors/{id}` now deletes the connector's slug-keyed `SecretStore` entry as well as the row. Previously the secret lingered after the row was removed, so a later connector created with the same slug could load the deleted instance's credentials. The secret is deleted *before* the row using a new `ConnectorSupervisor::secret_store()` accessor, and `SecretStore::delete` is idempotent (a missing entry is `Ok`), so an instance that never stored credentials cleans up as a no-op. A secret-deletion failure aborts the removal (`500`) and leaves the instance intact, so the database and secret store are never left in an ambiguous state and the request never reports success while a credential lingers. New route test: deleting an authenticated connector removes its credential, verified by loading it again and by re-creating a same-slug connector that cannot load the old secret.
- **Return the connector-not-found detail (functional correctness):** `KnowledgeError::ConnectorNotFound` was mapped to `404` but the message branch omitted it, so deleting an unknown connector returned the generic `"internal knowledge graph error"` instead of the not-found detail. It now preserves its detail (`"Connector {id} not found"`) like the other not-found variants. New unit test for the mapping and a route-level `DELETE /connectors/{id}` 404 test.
- **Docs:** updated stale `v0.89.0` release references to `v0.90.0` in `docs/wiki/connectors.md`, the `docs/wiki/what-works-now.md` version banner, and `README.md`; documented the secret-deletion flow in `docs/connector-management.md` and `docs/wiki/connectors.md`.

## [0.90.0] — 2026-08-06

### Connectors — PR #263 review fixes

- **Atomic connector creation (data integrity):** `POST /connectors` no longer pre-reads `get_connector_by_slug` then calls `upsert_connector` (a read-then-write window that let two concurrent same-slug writes both pass the read and let the later one update the first row instead of returning `409`). A new `KnowledgeGraph::create_connector` / `queries::connector::create_connector` does a plain `INSERT` with no `ON CONFLICT` clause and relies on the `connectors.slug UNIQUE` index to reject a duplicate at the database level, mapping the unique violation to a new `KnowledgeError::ConnectorSlugConflict` (mapped to `409 Conflict` by the server error layer, preserving the existing `connector slug '...' already exists` message). `upsert_connector` is retained for the A2 / #203 reconfigure-an-existing-instance flow. New tests: a KnowledgeGraph-level concurrent same-slug create asserting exactly one winner, and a route-level concurrent `POST /connectors` asserting exactly one `201` and one `409`.
- **`ConnectorSupervisor::stop` returns `false` for an already-finished runner (functional correctness):** previously `stop` removed a stale handle whose task had completed naturally (e.g. an unauthenticated connector whose runner exited at the auth handshake) and returned `true`, contradicting its own doc that `false` means "no live runner exists (already finished, never spawned, or previously stopped)". It now distinguishes a live runner (abort + await + `true`) from a finished one (clean up the stale handle + `false`); the `None` path is unchanged. The `MockConnector` gains an `auth_fail` config flag so the supervisor lifecycle tests can drive the already-finished-handle path.

### Docs

- `docs/connector-management.md`: corrected the `GET /connectors` list-route description (it uses one `count_sources_by_connector` `GROUP BY` query, not a per-row `count_sources_for_connector` query), documented `count_sources_by_connector` in the knowledge-graph additions, and updated the `POST` route notes, supervisor `stop` description, and tests section to reflect the atomic create and finished-runner behaviour.
- `docs/wiki/what-works-now.md`: fixed a broken code span in the Phase 3 entry (`GET/DELETE /connectors/{id}`).
- `docs/wiki/connectors.md`: noted that `POST /connectors` slug uniqueness is enforced atomically by the `slug UNIQUE` index.

## [0.89.0] — 2026-08-05

### Connectors — daemon wiring + connector CRUD/status routes (A1 / #202)

- **Daemon owns the connector framework at startup:** `AppState::from_config_with_llm` now constructs a `ConnectorRegistry`, registers the built-in Photos (`local`), Calendar (`caldav`), and Email (`imap`) factories behind forwarded cargo features on `mimir-server`, builds a `ConnectorSupervisor` subscribed to the daemon-wide shutdown watch, and chains `with_secret_store(FileSecretStore)` (best-effort), `with_geocoder` (the same `Arc<dyn Geocoder>` the knowledge graph holds), `with_user_identity(cfg.identity.name)` (C4 / #198), and `with_llm_backend(llm_client)` (C7 / #201 — enables the Email prose-extraction system-queue path). `Active` connector runners are restored at startup and drained on graceful shutdown via `AppState::shutdown`. `mimir-server` forwards `photos`/`calendar`/`gmail` features to `mimir-connectors` so each factory registration is gated by the same flag that compiles the backend module.
- **Connector CRUD/status routes:** four Axum routes round-trip via `mimir-client` — `GET /connectors` (list with derived item counts), `POST /connectors` (add-only; validates the `(connector_type, backend)` pair against the registry, rejects an existing slug with `409`, rejects an unregistered backend or unknown type with `400`, creates the instance in `Setup`), `GET /connectors/{id}` (show with item count; `404` when missing), and `DELETE /connectors/{id}` (stops the runner and deletes the row; `204`). Activation, pause/resume, OAuth, the `forget` cascade, and the `mimir connector` CLI land in A2–A4 (#203–#205).
- **`ConnectorSupervisor::stop(id)`:** per-instance counterpart of `shutdown()` — aborts a single runner and removes it from the handle map (a no-op returning `false` when no live runner exists). `DELETE` uses it so a mid-cycle sync cannot write back to a vanishing row.

### Knowledge graph — connector provenance support

- **`KnowledgeGraph::count_sources_for_connector(id)`:** the derived "items ingested" metric surfaced by the connector status routes (`SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?`). The `connectors` table stores no count column; the value is computed on demand.
- **`KnowledgeGraph::delete_connector(id)`:** nulls every `sources.connector_instance_id` referencing the row (the FK has no `ON DELETE` clause, so a raw `DELETE` would violate it) then deletes the row, in one transaction. Ingested facts survive with degraded provenance; the full `forget` cascade is deferred to A2 / #203. Returns `ConnectorNotFound` when no row matches. `ConnectorNotFound` is now mapped to `404` by the server error layer.

### API surface

- New wire types in `mimir-api-types`: `AddConnectorRequest`, `ConnectorResponse` (carries `item_count`, lowercase-string `status`/`auth_state`, RFC-3339 timestamps), `ConnectorListResponse`. `mimir-api-types` stays decoupled from `mimir-knowledge`, so the connector kind/status are strings mapped to enums in the route layer.
- New `mimir-client` methods: `connectors`, `connector`, `connector_add`, `connector_remove`.

### Docs

- New `docs/connector-management.md` (technical). Updated `docs/wiki/connectors.md` (Managing connectors section), `docs/wiki/server.md` (endpoints), `docs/wiki/what-works-now.md` (routes, version, Phase 3 status), `README.md`, and `Mimir-Implementation-Context.md`.

## [0.88.0] — 2026-08-05

### Connectors — Email LLM extraction (C7 / #201)

- **Email connector LLM extraction layer (cascade layer 3):** the IMAP Email connector's `extract()` cascade gains a third, last-resort layer for unstructured prose that deterministic layers cannot read — a dentist's "see you Tuesday 3pm" with no `.ics`, a flight confirmation in prose, a bank statement, a job offer. Layer 3 runs only for messages that layers 1 (iMIP invites / #200) and 2 (`schema.org` JSON-LD / #249) produced no facts for, so a deterministic layer that already read the email is never re-processed by the LLM (avoids duplicate extraction and bounds LLM cost). When no `LlmBackend` is injected (daemon wiring is #202), layer 3 is skipped, leaving deterministic extraction unchanged.
- **"Logic in Rust, not prompts" enforced for connector LLM data:** the LLM must call a strict `extract_email_facts` tool (a closed JSON Schema) and Rust validates the tool-call name (rejecting an unexpected function or a multi-call completion) and every field against the typed enums before building `NormalizedFact`s — entity types, temporal bounds, an `event_type` hint (mapped against the `EventType` enum, dropped if unrecognised, never trusted raw), recurrence, and the location overlay — reusing the shared `mimir-knowledge::extract` parsing helpers (DRY with the conversational `remember` path). User-scoped subjects are canonicalised to the injected `user_identity` so they resolve to the canonical user entity (matching the C4/#198 and #249 layers). All facts carry `source_type = Connector`, `extraction_method = LlmExtraction`, and the email's `UIDVALIDITY`-qualified IMAP UID as `raw_reference`. The body sent to the LLM is the first `text/plain` part (or HTML stripped of markup), capped at 8 KiB.
- **Deterministic spam pre-filter:** obvious bulk-marketing mail is skipped before any LLM call by a Rust `is_likely_spam` gate: a message is skipped when it carries a `List-Unsubscribe` header (the universal bulk-mail signal) or is sent from a pure marketing platform domain (mailchimp/hubspot/mailerlite/etc.). General-purpose ESPs that also deliver transactional mail (sendgrid/mailgun/postmark/amazonses) are skipped only with the unsubscribe signal, so statements, bookings, and offers routed through them still reach the LLM. The LLM only does fact extraction and returns an empty `facts` array for no-fact prose.
- **Retryable LLM-failure handling:** a queue-full, network, provider, or parse failure during prose extraction is propagated as a `ConnectorError` (via a new `From<LlmError> for ConnectorError`) and the affected raw email is re-staged in the connector buffer for the next extraction cycle, rather than being silently converted into an empty (successful) extraction that would lose the message forever (the buffer was drained and the IMAP cursor advanced). Deterministic facts already collected this cycle are kept, so a transient LLM error never blocks them; a durable terminal-failure policy is follow-up work.

### LLM backend — system-queue tool-calling (#201 acceptance)

- **`LlmBackend::system_chat_message` / `system_chat` trait methods:** every connector LLM call routes through the shared `LlmWorkerPool`'s **system queue** (priority below user chat), so a one-call-at-a-time provider is never starved by an extraction burst and a queued user chat preempts a waiting connector call. The default implementation delegates to the user-queue `chat_message` for backends without a pool (mocks, model-override clones, direct test clients). `LlmClient` overrides it to enqueue on the system queue. The system-queue enqueue methods (`enqueue_system_chat_message` / `enqueue_system_chat` / `enqueue_system_chat_stream`) now accept a `tools` argument — previously the system queue hardcoded `tools: None`, so connector tool-calling could not use the system queue. `MockLlmClient` records `system_chat_message` calls separately from user-queue `chat_message` calls so tests assert the routing directly.

### Knowledge graph — per-fact `ExtractionMethod` (#234)

- **`NormalizedFact::extraction_method` per-fact override:** `ExtractionMethod` moves onto `NormalizedFact` as an `Option<ExtractionMethod>` that defaults to (inherits) the batch `Provenance`'s method when unset. This lets a single mixed-method `extract()` batch record the right method per fact in `sources.extraction_method_id`, fixing the `ConnectorSupervisor::run_cycle` hardcoded `ExtractionMethod::StructuredParse` (#234 / #254) that would have mislabelled C7's LLM-extracted facts as structured-parsed. The email cascade tags its facts per layer: layers 1–2 set `StructuredParse`, layer 3 sets `LlmExtraction`. The conversational `remember` path tags its facts `LlmExtraction`. Existing producers are unchanged (inherit the provenance default).

### Connectors — shared LLM backend injection

- **`ConnectorContext::llm_backend` + `ConnectorSupervisor::with_llm_backend`:** the shared `Arc<dyn LlmBackend>` is injected into connectors at construction (mirroring the existing `with_geocoder` / `with_secret_store` / `with_user_identity` builders), and the Email connector clones it at construction. Daemon wiring that calls `with_llm_backend` lands with #202. No new dependencies: the LLM layer reuses `mimir-core::llm` and `mimir-knowledge::extract`; tests use `mimir-core`'s `mock-llm` feature (now a `mimir-connectors` dev-dependency).

### Docs

- `docs/email-connector.md`, `docs/wiki/email-connector.md`, `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md` updated for the C7 layer, the system-queue tool-calling, and the per-fact `ExtractionMethod`.

## [0.87.0] — 2026-08-04

### Bug fixes

- **Configurable knowledge-graph + job-queue database paths (#233):** `knowledge.db_path` and `scheduler.db_path` are now configurable (and overridable via `MIMIR_KNOWLEDGE_DB_PATH` / `MIMIR_JOBS_DB_PATH`), mirroring the existing `context.db_path`. Previously `AppState::from_config_with_llm` hardcoded the knowledge-graph and job-queue databases to the shared Mimir data directory, ignoring any config override, so the in-process e2e daemon opened the developer's real `~/.local/share/mimir/knowledge.db`. Knowledge-graph backups are now written alongside the (possibly overridden) knowledge DB instead of always escaping to the shared data dir.
- **E2E tests isolate every database to the tempdir (#251, #256, #237):** `mimir/tests/e2e.rs` now points the in-process daemon at tempdir paths for `context.db`, `knowledge.db`, and `jobs.db`. The suite no longer touches or migrates the developer's real knowledge/job databases, eliminating the `migration 45 was previously applied but has been modified` checksum-mismatch failure that made `e2e_ask_no_stream_round_trip` always fail on machines that had run `mimir start` before.
- **Fix flaky `entity_locations` "database is locked" (#236):** the background location-overlay worker and the ingestion caller (`normalize_and_insert`) now serialise their SQLite write transactions through a shared `KnowledgeGraph::write_lock`. In WAL mode a deferred read-then-write transaction that has another connection commit between its read and its write is rejected with an immediate, un-retriable `SQLITE_BUSY` (the snapshot is stale, so `busy_timeout` cannot help), which silently dropped a location overlay or failed an `insert_fact` under interleaving. The lock is held per-fact across the ingestion write and across the worker's `upsert_location` + `ensure_place_coordinates` writes, but not across the geocode network call, so off-thread geocoding throughput is preserved and reads stay fully concurrent.

### Docs

- `docs/config-system.md`, `docs/wiki/configuration.md`, `docs/entity-locations.md`, `docs/wiki/entity-locations.md`, and `docs/wiki/e2e-tests.md` updated for the new database-path overrides and the write-serialisation fix.

## [0.86.2] — 2026-08-04

### Connectors — Email JSON-LD extraction review follow-up (PR #257)

- **Functional correctness (textual lodging addresses):** `LodgingReservation` location extraction now accepts a scalar textual `address` value (schema.org permits `address` as plain `Text` in addition to a `PostalAddress` object). Previously `a.as_object()?` dropped scalar addresses so the lodging-name fallback discarded a valid `located_in` fact. Scalar addresses are now coerced via `scalar_string` before the `streetAddress` lookup, while structured `PostalAddress` handling and the self-referential `located_in` skip are preserved. A regression test covering a textual address that produces the expected `located_in` fact is added.

## [0.86.1] — 2026-08-04

### Connectors — Email JSON-LD extraction review follow-up (PR #257, #249)

- **Data integrity (reservation start-time gating):** the primary `Appointment`-typed JSON-LD reservation facts (`FlightReservation` `has_flight`, `LodgingReservation` `has_booking`, `EventReservation` `has_event`) now require a parseable start time (`departureTime` / `checkinDate` / `startDate`) before emission, matching the iMIP layer's `DTSTART` requirement so the events subsystem cannot create an appointment overlay without a `valid_from`. Secondary facts (airports, airlines, venues) are still always emitted.
- **Data integrity (self-referential `located_in`):** `LodgingReservation` no longer emits a `located_in` fact when the resolved location equals the booking name (e.g. `Grand Hotel located_in Grand Hotel`); a distinct address is required.
- **Functional correctness (numeric identifiers):** `string_or_name_field` and the `iataCode` / `flightNumber` lookups now accept JSON numbers (producers commonly emit `orderNumber` / `trackingNumber` / `ticketNumber` as numbers) and trim array-wrapped string values, via a new shared `scalar_string` helper. Numeric `orderNumber`, `trackingNumber`, `ticketNumber`, `flightNumber`, and `iataCode` no longer drop their fact cluster.
- **Functional correctness (datetime parsing):** `parse_datetime` now accepts naive datetimes with fractional seconds (`...T10:00:00.500`) and minute-only precision (`...T10:00`) in addition to RFC 3339, second-precision naive, and date-only inputs.
- **HTML scanner (spec compliance):** `<script type="...">` attribute values are now trimmed before the `application/ld+json` comparison, matching the HTML5 rule that browsers strip ASCII whitespace from the `type` attribute.
- **Encapsulation:** `mimir-connectors/src/email/mod.rs` narrows `pub mod jsonld;` to `pub(crate) mod jsonld;` (the module has no out-of-crate references).
- **Docs:** `docs/email-connector.md` and `README.md` updated (cascade layer count corrected; C7 / #201 marked as still open; emission rules and numeric/datetime handling documented). A known limitation — iMIP + JSON-LD both firing on one email can produce duplicate `Event` entities — is now documented inline in `extract()`.
- **Tests:** new unit tests cover the start-time gating (flight/lodging/event), the self-referential `located_in` skip, numeric identifiers (order/tracking/ticket/flight/iata), trimmed array values, padded `type` attribute, and the fractional/minute-only datetime shapes.

## [0.86.0] — 2026-08-04

### Connectors — Email schema.org JSON-LD deterministic extraction (#249)

- **New extraction layer:** `EmailConnector::extract()` gains a second deterministic (structured-parse) layer that scans `text/html` MIME parts for `<script type="application/ld+json">` blocks and extracts typed fact clusters for recognised `schema.org` types. This sits as layer 2 of the extraction cascade, between the iMIP calendar-invite layer (layer 1, #200) and the C7 LLM layer (#201, still open). No LLM — pure Rust parsing per the project rule "logic in Rust, not prompts".
- **Recognised types:** `FlightReservation` (`user has_flight <flight>` typed `EventType::Appointment`, plus `departs_from` / `arrives_at` / `operated_by`), `LodgingReservation` (`has_booking`, `Appointment`, `located_in`), `EventReservation` (`has_event`, `Appointment`, `located_in`), `Order` (`has_order`, plus `purchased_from`), `ParcelDelivery` (`has_delivery` typed `Reminder`, plus `shipped_by` / `delivered_to`), `Ticket` (`has_ticket`, plus `issued_by`), and `ReservationPackage` (flattens `subReservation` for multi-leg flights). Unrecognised `@type` values are logged at `debug` level and skipped — never guessed.
- **Provenance and user identity:** facts carry `source_type = Connector`, `extraction_method = StructuredParse`, and the email's `UIDVALIDITY`-qualified IMAP UID as `raw_reference` (matching the iMIP layer). The primary user-scoped fact is only emitted when a canonical user identity is configured (`ConnectorContext::user_identity`); secondary facts (airports, airlines, venues, carriers, merchants) are always emitted. Duplicate/re-sent transactional emails dedupe via the existing `normalize_and_insert` corroboration/supersession.
- **New module:** `mimir-connectors/src/email/jsonld.rs` (gated by the `gmail` feature). The HTML `<script>` scanner is hand-rolled (spec-correct: HTML5 script content terminates at the first `</script>` end tag) — no new HTML parser dependency. The shared `vevent_fact` helper in `ical.rs` is made `pub(crate)` for DRY reuse (JSON-LD facts delegate to it with `RecurrenceType::None`).
- **No new dependencies:** the module reuses the existing `serde_json`, `mail-parser`, and `chrono` crates.
- **Tests:** unit tests for each recognised type, the HTML `<script>` scanner (standard, single-quoted, multi-attribute, case-insensitive, JavaScript-skipping, `data-type`-not-`type`), JSON-LD structural normalization (`@graph`, arrays, context wrappers), a cascade integration test (iMIP + JSON-LD from one email), and a KB integration test (flight fact entity resolution + `Appointment` events-subsystem overlay + connector provenance).
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, `docs/wiki/what-works-now.md`, and `README.md` updated.

## [0.85.2] — 2026-08-04

### Connectors — Email iMIP extraction review follow-up 2 (PR #253, #200)

- **Data integrity (iMIP `METHOD` conflict):** `EmailConnector::extract_invites` (`mimir-connectors/src/email/mod.rs`) now normalises the MIME `Content-Type` `method` parameter and the iCalendar body `METHOD` property independently, and rejects (skips) a `text/calendar` part when both are present and disagree instead of silently preferring the MIME value. This prevents appointment facts being created from a part whose body says `METHOD:CANCEL`/`METHOD:PUBLISH` while the MIME header claims `REQUEST`/`REPLY` (RFC 6047 §2.4 requires the two to match when both are supplied). Matching values and single-source values keep their existing behaviour; `PUBLISH` and `CANCEL` remain skipped. New regression tests cover conflicting supported/unsupported pairs, the body-only fallback, and the no-source case.
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, and `README.md` updated to match the extraction contract — eligible parts are any `text/calendar` MIME part (not just attachments), `METHOD` is resolved from the MIME parameter or the calendar body, the `has_event` primary fact depends on a configured `ConnectorContext::user_identity`, supported `REQUEST`/`REPLY` invitations produce facts regardless of sender, and provenance is the `UIDVALIDITY`-qualified IMAP UID.

## [0.85.1] — 2026-08-04

### Connectors — Email iMIP extraction review follow-up (PR #253, #200)

- **Functional correctness (iMIP MIME walk):** `EmailConnector::extract_invites` (`mimir-connectors/src/email/mod.rs`) now walks every MIME part (`message.parts`) instead of `attachments()`, so a `text/calendar` part nested in `multipart/alternative` with no `Content-Disposition: attachment` header (classified as a body part by `mail-parser`) is no longer missed.
- **Functional correctness (METHOD fallback):** the iMIP `METHOD` is now resolved from the MIME `Content-Type` `method` parameter when present, falling back to the iCalendar body `METHOD` property (RFC 6047 §2.4 makes the parameter optional). Only `REQUEST`/`REPLY` are extracted; `PUBLISH`/`CANCEL` are still skipped.
- **Data integrity (globally-unique provenance):** the email provenance `raw_reference` is now `{uid_validity}:{uid}` (matching the persisted cursor format), not a bare IMAP UID that is unique only within one mailbox + `UIDVALIDITY` epoch. `imap::RawEmail` gains a `uid_validity` field populated by `fetch_since`.
- **Performance (buffer lock):** `EmailConnector::extract()` now drains the staged buffer and releases the mutex guard before the CPU-bound MIME parse loop, so a concurrent `sync()` cycle is not blocked from staging new mail during parsing.
- **Data integrity (iCalendar name matching):** `parse_ical_to_vevents` (`mimir-connectors/src/ical.rs`) now looks up VEVENT properties case-insensitively (`icalendar` 0.17.x `find_prop` matches case-sensitively, but RFC 5545 names are case-insensitive), and the `VEVENT` component match is case-insensitive. `participant_display` strips the `mailto:` scheme case-insensitively (`MAILTO:` occurs in the wild, per RFC 3986).
- **Tests:** the iMIP invite fixture now uses CRLF line endings (RFC 5322/MIME + IMAP `BODY.PEEK[]` wire format); the tautological `assert!(dr_smith > 0)` is replaced by a real check that the Dr Smith `attending` fact resolved to the right Person entity and points at the appointment Event; the fake-IMAP → `extract()` provenance assertion updated for the `UIDVALIDITY`-qualified `raw_reference`.
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, and `README.md` corrected — `mail-parser` is Gmail-specific while `icalendar`/`chrono-tz` are shared with `calendar`; the wiki states the `has_event` primary fact depends on a configured `user_identity`, that only supported iMIP `REQUEST`/`REPLY` parts produce facts, and that provenance is the IMAP UID; the README splits future-work references (`schema.org` JSON-LD = #249, LLM free-text extraction = C7 / #201).

## [0.85.0] — 2026-07-31

### Connectors — Email structured extraction (C6 / #200)

- **Structured extraction cascade:** `EmailConnector::extract()` (`mimir-connectors/src/email/mod.rs`) now drains staged RFC 822 messages and runs a deterministic extraction cascade over each. Today the cascade has one layer — iMIP calendar invites: a MIME attachment with `Content-Type: text/calendar; method=REQUEST|REPLY` is parsed with `mail-parser` and the embedded VEVENT is turned into the same appointment fact cluster the Calendar connector emits (`user has_event <event>` typed `EventType::Appointment`, recurrence from `RRULE` `FREQ`, temporal bounds from `DTSTART`/`DTEND`, plus `<event> located_in <place>` and `<attendee> attending <event>`). `method=PUBLISH` (often marketing webinars) and `CANCEL` (deletion lifecycle, tracked in #247) are skipped. A plain prose email with no `text/calendar` part produces no facts.
- **Email is provenance, not the fact:** no per-email communication facts (`received_email_from` / `sent_email_to`) are emitted and no `Person` entities are auto-created from `From`/`To` headers, so marketing/spam produces no junk. The email's IMAP UID rides on every fact as the `raw_reference`; facts carry `source_type = Connector`, `connector_type = Gmail`, `extraction_method = StructuredParse`.
- **DRY — shared `ical` module:** the VEVENT parsing + fact cluster is extracted into a new `mimir-connectors/src/ical.rs` (`parse_ical_to_vevents`, `vevent_to_facts`, `RawVEvent`), gated `any(feature = "calendar", feature = "gmail")`. The Calendar connector (`caldav.rs` `RawCalDavEvent` now wraps a `RawVEvent`; `calendar/mod.rs` `event_to_facts` delegates) reuses it, eliminating the duplicated VEVENT parsing + `rrule_to_recurrence` + `calendar_fact` helpers.
- **User identity:** the Email connector now authors user-scoped facts against the injected `ConnectorContext::user_identity` (the `config.toml` `[identity] name`), matching the Calendar connector; `from_config_with_http` gains a `user_identity` parameter and the factory passes `ctx.user_identity`. Without an identity the primary `has_event` fact is skipped; location/attendee facts still emit.
- **Dependencies:** `mail-parser 0.11.5` added to `mimir-connectors` under the `gmail` feature; `icalendar` 0.17 and `chrono-tz` 0.10 are now shared with the `calendar` feature (DRY) so the `gmail` feature can parse iMIP VEVENTs.
- **Tests:** unit tests for iMIP `REQUEST`/`REPLY`/`PUBLISH`/`CANCEL` gating, the no-identity path, and plain/marketing email → no facts; a knowledge-graph integration test staging an invite through `extract()` → `normalize_and_insert` asserting F5 entity resolution (user / event / place / attendees), the `Appointment` events-subsystem overlay, secondary facts carrying no overlay, and connector provenance; a fake-IMAP → `extract()` round-trip proving the transport and extraction compose. Calendar tests updated for the shared `RawVEvent` (`.vevent.` field access) and remain green.
- **Follow-ups:** deterministic `schema.org` JSON-LD extraction for transactional email is #249; LLM extraction for free-text prose (flights/bookings/confirmations) is C7 / #201; the Photos coords-only `took_photo` provenance-as-fact fallback is #250.
- **Docs:** `docs/email-connector.md`, `docs/wiki/email-connector.md`, `README.md`, `docs/wiki/what-works-now.md`, and `Mimir-Implementation-Context.md` updated for C6.

### Connectors — Calendar review follow-up (PR #248, #198)

- **Data integrity (recurring facts):** `CalendarConnector::event_to_facts` (`mimir-connectors/src/calendar/mod.rs`) no longer sets `valid_until = DTEND` on a recurring `has_event` fact. A `RRULE:FREQ=WEEKLY` standup previously got a validity window of minutes (the first instance's `DTSTART`→`DTEND`), so current-facts reads and supersession keyed on `valid_until` treated it as long expired even though the events-subsystem overlay kept recurring. `valid_until` is now left unset when `RRULE` `FREQ` is present; a one-time event still carries its `DTEND` bound. New tests lock in both branches (`extract_one_time_event_carries_dtend_as_valid_until`, recurring `valid_until == None`).
- **Security (write-back href guard):** `CalendarConnector::act` now validates every `create_event` / `update_event` / `delete_event` `href` against the configured `calendar_url` origin (scheme + host + port + collection path) before issuing the CalDAV request, so a caller-supplied URL cannot redirect the stored Basic/Bearer credentials to another host (or an unrelated resource on the same host). An out-of-bounds `href` returns `ConnectorError::Config` with no request sent. Tests: `write_back_rejects_href_outside_calendar_origin`, `write_back_rejects_href_with_wrong_path_on_same_origin`.
- **Correctness (DST fold):** `parse_ical_datetime` (`mimir-connectors/src/calendar/caldav.rs`) now prefers the earliest offset for an ambiguous autumn-fold local time (`zone.from_local_datetime(&naive).earliest()`) before the naive-as-UTC fallback, keeping the event within an hour of the wall clock instead of shifting it by the full zone offset. A spring-forward gap still hits the fallback. Test: `parse_ical_datetime_tzid_autumn_fold_prefers_earliest_offset`.
- **Data integrity (identity trimming):** the canonical user identity is now stored trimmed at every injection site (`ConnectorContext::with_user_identity`, `ConnectorSupervisor::with_user_identity`, and the Calendar connector constructor), so a padded `[identity] name` flows through as the canonical name rather than creating a duplicate person entity. Tests: `context_with_user_identity_trims_surrounding_whitespace`, `extract_trims_padded_user_identity`.
- **Docs:** corrected the per-VEVENT fact cardinality in `docs/calendar-connector.md` (one primary + optional location + one per attendee, not "up to three"); removed stale pre-C4 status statements in `docs/wiki/calendar-connector.md`; documented the write-back href guard; aligned `docs/wiki/what-works-now.md` to 0.84.2 with 0.84.1/0.84.2 release notes; and reworded the `user_identity` doc blocks to match the actual no-identity behaviour (the primary fact is skipped, not an "Event-centric fallback").

## [0.84.1] — 2026-07-30

### Connectors — Calendar secondary-fact overlay fix (review follow-up, #198)

- **Data quality:** `CalendarConnector::event_to_facts` (`mimir-connectors/src/calendar/mod.rs`) no longer sets `valid_from`/`valid_until` on the secondary `located_in` and `attending` facts. Previously these inherited the event's `DTSTART`/`DTEND`, so for a future-dated event `event_from_extraction` (`mimir-knowledge/src/normalize.rs`) spawned a spurious `Reminder` events-subsystem overlay keyed on each secondary fact's id — a location/attendance relationship is not an event, and the module doc states only the primary `has_event` fact should drive the overlay. The secondary facts now carry no temporal bounds, so they spawn no overlay; the primary `has_event` fact (typed `EventType::Appointment`) remains the sole overlay source. `calendar_fact` now takes `valid_from: Option<DateTime<Utc>>` to express this. New assertions in `calendar_sync_surfaces_upcoming_event_for_user` (no overlay on the `located_in` fact) and `extract_emits_event_location_attendee_facts_with_identity` (secondary facts have `valid_from`/`valid_until = None`) lock in the behaviour.

## [0.84.0] — 2026-07-30

### Connectors — Calendar event extraction + write-back (Phase 3 C4 / #198)

- **Event → KB fact extraction:** `CalendarConnector::extract` (`mimir-connectors/src/calendar/mod.rs`) now drains staged VEVENTs into a cluster of `NormalizedFact`s through the shared `normalize_and_insert` pipeline. Per VEVENT it emits a primary `user has_event <event>` (typed `EventType::Appointment`, recurrence mapped from `RRULE` `FREQ`), `<event> located_in <place>`, and `<attendee> attending <event>`; every subject/object resolves to an entity via the full F5 chain (exact → alias → FTS5 fuzzy → create), and future-dated/recurring events surface in the user's "Upcoming" memory section (#74). The event entity is named by the `SUMMARY` (fallback `UID`); locations resolve to `Place` entities and attendees to `Person` entities (the `CN` parameter, else the `mailto:` value).
- **User identity injection:** `ConnectorContext` gains a `user_identity: Option<String>` field (`with_user_identity` builder) and `ConnectorSupervisor::with_user_identity` injects the canonical `config.toml` `[identity] name` so the connector authors `user has_event <event>` against the same entity the daemon resolves as `user_entity_id` (and the event surfaces in the user-scoped Upcoming section). When no identity is configured the primary fact is skipped; location/attendee facts are still emitted. This supersedes the Photos connector's disconnected per-instance `owner_name` (aligning it is tracked as a follow-up).
- **`event_type` hint on `NormalizedFact`:** `mimir-knowledge::normalize::NormalizedFact` gains `event_type: Option<EventType>`; `event_from_extraction` honours it when present, falling back to the existing `Task`/`Reminder` derivation for chat (behaviour unchanged). The Calendar connector sets `Appointment`.
- **iCalendar date + recurrence parsing:** `parse_icalendar` (`mimir-connectors/src/calendar/caldav.rs`) now parses `DTSTART`/`DTEND` to UTC at staging time — UTC (`…Z`), floating local, date-only, and `TZID`-qualified values (resolved via the new `chrono-tz` 0.10 dependency; an unknown zone falls back to the naive value as UTC so a bad `TZID` never drops the event) — and captures attendees/organizer. A new `rrule_to_recurrence` maps `RRULE` `FREQ` to the coarse `RecurrenceType` (full RFC 5545 `COUNT`/`UNTIL`/`INTERVAL`/`BYxxx` out of scope). `RawCalDavEvent` is reshaped: typed `starts_at`/`ends_at: Option<DateTime<Utc>>` replace the raw `dtstart`/`dtend` strings, plus `attendees: Vec<String>` and `organizer: Option<String>`. The now-unused `raw_ical` payload field is dropped (C4's extractor works from the parsed fields, so retaining the full calendar text per staged event was dead weight).
- **CalDAV write-back:** `CalDavClient` gains `put_event` (RFC 4791 §5.5, `If-None-Match: *` for create / `If-Match: <etag>` for update) and `delete_event` (§5.6, idempotent on 404). `CalendarConnector::act` implements `create_event` / `update_event` / `delete_event`, building VEVENTs with the `icalendar` builder and a `uuid`-generated `UID`. This is the only connector with write support.
- **Dependencies:** `chrono-tz` 0.10 and `uuid` 1 (`v4`) added to `mimir-connectors` under the `calendar` feature. `uuid` is already in the tree transitively; `chrono-tz` is a new download.
- **Tests:** new caldav unit tests (iCalendar datetime UTC/date-only/floating/TZID-DST resolution, attendee/organizer extraction) and integration tests (extraction fact shape with/without identity, recurrence mapping, write-back create/update/delete against a mock CalDAV server, idempotent 404 delete, unsupported-action error, and a full sync → `normalize_and_insert` → "Upcoming" round-trip proving the `Appointment` overlay). Existing C3 tests updated for the reshaped `RawCalDavEvent`.
- **Docs:** `docs/calendar-connector.md`, `docs/wiki/calendar-connector.md`, `docs/wiki/what-works-now.md`, `docs/wiki/events-and-reminders.md`, `README.md`, and `Mimir-Implementation-Context.md` updated for C4.

## [0.83.3] — 2026-07-30


### Connectors (review follow-up, PR #244)

- **Security (token endpoint):** `oauth::refresh_token` loopback detection (`mimir-connectors/src/oauth.rs`) now strips the surrounding `[...]` brackets that `Url::host_str` adds to IPv6 hosts before parsing them as `std::net::IpAddr`, so an `http://[::1]:<port>/token` loopback endpoint is correctly accepted instead of being over-rejected as non-loopback. The detection was extracted into a unit-testable `is_loopback_url` helper; new tests `is_loopback_url_accepts_ipv6_loopback`, `is_loopback_url_rejects_lookalike_and_remote_hosts`, and `refresh_token_accepts_ipv6_loopback_endpoint` cover the regression, while the existing `refresh_token_rejects_lookalike_loopback_host` test confirms `127.0.0.1.evil.com` stays rejected.
- **Docs:** the `0.83.2` CHANGELOG `Code quality` bullet for `EmailConnector::resolve_auth` was reworded to Rust-accurate language (a `String` has no falsy state; the behaviour is explicit handling of a missing `access_token`). `docs/wiki/what-works-now.md` version field aligned to `0.83.3`.

## [0.83.2] — 2026-07-28

### Connectors (review follow-up, PR #244)

- **Data integrity:** `ImapConnector::examine` (`mimir-connectors/src/email/imap.rs`) now returns a `ConnectorError::Parse` when the server omits the `UIDVALIDITY` response code instead of collapsing to epoch `0`, which could collide with a persisted `0:<uid>` cursor and silently skip mail (RFC 3501 mandates the response code). New test `missing_uidvalidity_is_an_error_not_zero` (fake-IMAP server gains an `omit_uid_validity` flag).
- **Stability:** `EmailConnector::use_idle` now returns `Result<bool, ConnectorError>` and the `Auto` mode defaults to IDLE (`true`) when unprobed, matching `Connector::mode` (which reports `Push` for `Auto` + `None`); previously `use_idle` defaulted to `Polling` (`false`), so a pre-probe `Auto` connector reported `Push` but polled immediately, busy-looping the supervisor. `Idle` mode now errors with `ConnectorError::Config` when the cached capability confirms the server lacks `IDLE`, honouring the documented "error if the server lacks the capability" contract. New tests `forced_idle_errors_when_server_lacks_idle` and `auto_mode_defaults_to_push_when_unprobed_matching_mode`.
- **Security (token endpoint):** `oauth::refresh_token` (`mimir-connectors/src/oauth.rs`) now rejects non-HTTPS token endpoints before posting the `refresh_token` / `client_secret`, allowing loopback HTTP only (`127.0.0.1` / `::1` / `localhost` — Mimir's local trust boundary, where credentials never traverse a network). New tests `refresh_token_rejects_non_https_endpoint` and `refresh_token_rejects_unparseable_endpoint`.
- **Security (error hygiene):** provider-supplied `error_description` in OAuth token-error responses is now truncated to 256 bytes (on a UTF-8 boundary, marked with an ellipsis) so an unbounded provider payload cannot bloat logs / the persisted `last_error`; the sanitisation promise ("only parsed `error`/`error_description`, never the raw body") is now size-bounded. New tests `truncate_description_*` and `token_error_message_truncates_a_long_error_description`.
- **Code quality:** `EmailConnector::resolve_auth` OAuth-refresh missing-`access_token` handling simplified to `ok_or_else` (matching `CalendarConnector::resolve_auth`), now handling a missing `access_token` explicitly via `ok_or_else` instead of the prior `unwrap_or_else(String::new)` + `is_empty` check (a Rust `String` has no falsy state, so the old wording overstated what `is_empty` detected). Fixed broken intra-doc link `ImmapAuth` → `ImapAuth` (`email/imap.rs`) and replaced the private `oauth` module's `[`oauth`]` intra-doc link in the crate-level docs with a code span to avoid `private_intra_doc_links`.
- **Docs:** `docs/email-connector.md` no longer documents `client_secret` in the `config_json` credential description (public-client PKCE only; confidential-client credentials never leave the `SecretBundle::OAuth` boundary). `docs/wiki/connectors.md` counts three concrete backends, drops completed IMAP ingestion from the planned section (only C6/C7 extraction remains), and places the `## How connector credentials are stored` heading on its own line so the in-page anchor resolves. `docs/wiki/email-connector.md` qualifies "Forget everything from Gmail" as a library-level capability. `docs/wiki/what-works-now.md` version field aligned to `0.83.2`. Newly touched Markdown prose (CHANGELOG `0.83.0` bullets, email-connector status/C5-C7 blockquotes, what-works-now release summary) reflowed onto single physical lines per the repo-wide prose rule.

## [0.83.1] — 2026-07-28

### Docs

- Remove duplicated doc-comment block on `EmailConnector::supports_idle` field (`mimir-connectors/src/email/mod.rs`). Code-review finding: the field comment contained a stale copy of its own first two lines, which has been collapsed into a single, accurate comment.

## [0.83.0] — 2026-07-28

### Connectors (Phase 3 C5 / #199)

- **New backend — IMAP Email connector** (`mimir-connectors`, feature `gmail`): the third concrete connector backend (after Photos and Calendar). An `async-imap` 0.11.3 client (built `default-features = false, runtime-tokio` to avoid pulling `async-std`) speaks IMAP over a hand-rolled TCP + `tokio-rustls` handshake — the workspace keeps a single rustls TLS stack instead of async-imap's `connect()` / `async-native-tls`. Login is `LOGIN` (app password) or `AUTHENTICATE XOAUTH2` (Google / Microsoft OAuth, with the SASL initial response `base64("user=<u>\x01auth=Bearer <t>\x01\x01")`).
- **Push + polling:** runs in `Push` (IMAP IDLE) mode when the server advertises `IDLE`, falling back to `Polling` otherwise — auto-detected via a `CAPABILITY` probe in `authenticate`/`health` (so `Connector::mode` returns the right value, called by the supervisor after `authenticate`). The `mode` config (`auto` | `idle` | `poll`, default `auto`) can force one.
- **Incremental sync:** `UID FETCH <last+1>:* (UID INTERNALDATE BODY.PEEK[])` with a UIDVALIDITY-safe `<uid_validity>:<last_uid>` cursor — a UIDVALIDITY mismatch on `EXAMINE` (mailbox recreated) triggers a full re-fetch, so a bare last-UID never silently gaps or duplicates. `BODY.PEEK[]` keeps mail unread. The cursor persists across restarts via the supervisor's `update_sync_cursor`.
- **Transport-only:** `extract()` stages raw RFC 822 messages and returns no `NormalizedFact`s yet. Mail parsing + structured fact extraction (headers/dates/contacts) is C6 (#200); LLM extraction (flights/bookings) is C7 (#201).
- **DRY OAuth refresh:** the Calendar connector's hand-rolled OAuth token-refresh + secret-safe error reporting moved into a shared `mimir-connectors::oauth` module; both the Calendar and Email OAuth connectors now share one implementation (avoids the reqwest-0.12-duplicating `oauth2` crate). No behaviour change to the Calendar connector; its refresh/error unit tests moved with the code.
- **Spec corrections vs. issue #199:** `async-imap 0.11.3` (not 0.11.2); rustls (not async-native-tls); UIDVALIDITY-encoded cursor (not bare last-UID). See `docs/email-connector.md`.
- **Tests:** unit tests for config/cursor/mode/auth resolution plus a fake-IMAP integration suite over a `tokio::io::duplex` pair (no TLS, no live account) covering app-password + XOAUTH2 login, IDLE push → fetch, IDLE timeout, polling incremental / no-op / full sync, and UIDVALIDITY reset. All deps already in the tree via reqwest/async-imap — no new downloads.

### Process

- **Markdown prose formatting rule:** `AGENTS.md` (Finishing Work) now requires flowing single-line prose in all repo `.md` files (README, CHANGELOG, docs/, docs/wiki/, …) and in PR descriptions and commit messages — no manual hard-wrapping with newlines inside paragraphs or list items (single newlines render as soft breaks and make raw text and diffs hard to read). A repo-wide retroactive reflow of the remaining hard-wrapped docs is tracked in #245.

## [0.82.1] — 2026-07-28 — 2026-07-28

### Docs

- **Review follow-up (PR #242):** fixed two documentation punctuation/clarity
  nits flagged by CodeRabbit review. `docs/calendar-connector.md`: added the
  missing comma before "so" and removed the unnecessary comma before "because"
  in the OAuth error-handling paragraph. `docs/wiki/what-works-now.md`:
  capitalised the sentence boundary ("The connector secret store …") in the
  Phase 3 roadmap summary. No code or behaviour change.

## [0.82.0] — 2026-07-27

### Connectors (Phase 3 C3 / #197)

- **New backend — CalDAV Calendar connector** (`mimir-connectors`, feature
  `calendar`, `Polling` mode): the second concrete connector backend. A
  `CalDavClient` speaks CalDAV over the existing `reqwest` 0.13 — PROPFIND
  (Depth 0 `resourcetype`) for calendar/health verification and a
  `sync-collection` REPORT (RFC 6578; root element in the `DAV:`
  namespace per §3.1, `calendar-data` in the CalDAV namespace) for event sync,
  requesting
  `<cal:calendar-data/>` inline so changed VEVENTs and a new `sync-token`
  arrive in one round trip. Omitting the sync-token does a full sync and yields
  the initial token; including it does an incremental sync (no full re-fetch),
  so the persisted sync-token is the connector's incremental cursor.
  `icalendar` parses each VEVENT (UID/summary/DTSTART/DTEND/location/status/
  RRULE) into a staged `RawCalDavEvent`; `roxmltree` parses the WebDAV XML by
  local tag name (namespace-prefix tolerant).
- **Auth:** app password (HTTP Basic — iCloud/Fastmail/Nextcloud) or an OAuth
  bearer token (Google) that the connector **refreshes** when expired (within a
  60 s skew) and persists back to the `SecretStore`. The interactive PKCE login
  that *obtains* the first OAuth token is deferred to A4 / #205; #197 only
  consumes + refreshes a stored token.
- **Framework:** this is the first backend that needs credentials, so
  `ConnectorContext` gained a `secret_store: Option<Arc<dyn SecretStore>>`
  field and `ConnectorSupervisor::with_secret_store(store)` (a breaking
  internal construction-context change, allowed by the project's
  breaking-changes policy). `ConnectorContext::with_secret_store` /
  `with_geocoder` builders added; `SecretStore` gained a `Debug` superbound
  (consistent with `Geocoder`).
- **Boundary:** `extract()` returns no facts yet — C3 is transport-only. C4 /
  #198 implements event → KB fact extraction + events-subsystem (#74)
  integration + write-back (`act`). Write-back is intentionally not in #197.
- **Dependencies:** `icalendar = "0.17"` (resolves to 0.17.6 under the workspace
  MSRV 1.85; 0.17.12 requires Rust 1.88 — see follow-up issue) + `roxmltree =
  "0.21"`, both gated by `calendar`. The `form` feature was added to the
  workspace `reqwest` for the OAuth refresh token POST. The `oauth2` crate is
  **deliberately not** pulled in (it depends on `reqwest` 0.12, duplicating the
  stack, and #197 only needs the refresh grant) — deferred to A4 / #205.
- **Tests:** unit tests for the CalDAV transport (sync-collection full/
  incremental parse, 401 handling, PROPFIND resourcetype detection, icalendar
  field extraction + recurrence, invalid-payload resilience) against a
  `wiremock` mock server, plus integration tests (app-password sync,
  incremental sync-token, `full`-sync cursor reset, OAuth refresh-on-expiry +
  bundle persistence, health states, factory construction, config round-trip,
  and a full `ConnectorSupervisor` round-trip asserting the cursor is
  persisted). No `unsafe`.
- **Review-driven hardening (PR #242):** the `sync-collection` request now
  carries the required `<d:sync-level>1</d:sync-level>` element (RFC 6578
  §6.3) and handles truncated (`507`) responses by paging with the advancing
  sync-token. A `<response>` with no `calendar-data` is tombstoned **only** on
  an explicit `404`/`410`; `403`/`423`/`507`/… are logged and skipped so a
  transient error never purges a live event. The XML helper concatenates all
  direct text/CDATA children (not just the first). OAuth hardening: an unknown
  `expires_at` no longer forces a refresh every cycle; `into_bundle` retains
  the prior refresh token when the response omits one; token-endpoint error
  bodies and the OAuth `client_secret` are never surfaced into persisted/logged
  `ConnectorError` strings (parsed `error`/`error_description` only; auth-kind
  discriminant used instead of `Debug`). `SecretBundle` gains a redacted
  `Debug` impl so printing a store/context never emits plaintext credentials.

## [0.81.2] — 2026-07-26


### Connectors / Knowledge (Phase 3 C2 / #196 review follow-up)

- **Stability:** `PhotosConnector::extract` now bounds geocode retries to one
  attempt per GPS bucket per `extract()` cycle via a per-cycle failed-key set.
  A sustained geocoder outage previously re-ran the geocoder's internal
  retry/backoff once per photo (not per distinct spot), stalling sync at
  ~1 req/s; now it degrades quickly to the coords-only fallback. The set is
  local to one cycle, so the next sync retries afresh — only success/no-match
  outcomes persist in the long-lived coord-dedup cache. New unit test
  `extract_bounds_geocode_retries_to_one_per_spot_per_cycle`.
- **Data integrity:** the single-`Geographic`-row-per-place invariant is now
  enforced at the schema level by a partial unique index
  (`idx_entity_locations_geographic_unique`, migration `047`) on `entity_id`
  scoped to `location_type_id = 6`. `ensure_place_coordinates` is a single
  atomic `INSERT ... ON CONFLICT DO UPDATE` against the index, so the
  read-then-write no longer relies solely on the serial overlay-worker
  convention. The index is deliberately partial — `Visited`/`Home`/`Work`/
  `Origin`/`Current` rows are not unique per `(entity_id, location_type_id)`.
  New integration test `ensure_place_coordinates_keeps_single_geographic_row`
  (sequential + concurrent). A `const_assert` locks `LocationType::Geographic
  == 6` since the SQL hardcodes the literal.
- **Observability:** `normalize_and_insert` now logs (debug) when a place is
  created but not anchored because no coordinates resolved, instead of silently
  no-op'ing.
- **Docs:** MD022 blank lines around anchored headings in
  `docs/photos-connector.md`; renamed the stale "What's next" heading in
  `docs/wiki/photos-connector.md` to "Location enrichment"; documented the
  per-cycle retry bound and the schema-level place-anchor invariant in
  `docs/photos-connector.md` and `docs/entity-locations.md`.

## [0.81.1] — 2026-07-26

### Connectors (Phase 3 C2 follow-up / #196)

- **Fix:** `PhotosConnector::extract` no longer holds the staged-photo buffer
  mutex across the per-photo reverse-geocode network awaits. The buffer is
  `std::mem::take`-drained into a local `Vec` under the lock and the guard is
  dropped before the geocode loop, restoring the C1 lock hold-time
  (in-memory map only) so `forget`/`reset` or a concurrent admin-triggered sync
  is not blocked for the ~N-second scan duration. No semantic change; existing
  unit and integration tests pass unchanged.

## [0.81.0] — 2026-07-26

### Connectors (Phase 3 C2 / #196)

- **Photos GPS → place extraction:** the local-filesystem Photos connector now
  reverse-geocodes each photo's EXIF GPS into a locality-level place name via
  the shared `Geocoder`, emitting `owner took_photo_at <place>` facts whose
  place is a `Place` object entity. Photos at the same place corroborate into
  one open-ended fact (+0.05/source, capped 0.95; base confidence 0.80), so the
  knowledge graph grows with distinct places visited, not photo count. A
  coord-dedup cache (~111 m buckets) bounds geocode calls to one per shooting
  spot; transient network errors aren't cached. When no place resolves (no
  geocoder / no match / transient error), the photo degrades to the C1
  coords-only `took_photo <rel_path>` shape so no data is lost.
- **Geocoder injection:** a new `ConnectorContext` (shared-services struct) is
  threaded factory → registry → supervisor. `ConnectorFactory::create` now
  receives `&ConnectorContext`; `ConnectorRegistry::create_with_context`
  forwards it; `ConnectorSupervisor::with_geocoder` sets the context's geocoder.
  No new dependencies (reuses the S1 `Geocoder`).
- **Place-coordinate anchoring:** two `entity_locations` rows per place fact —
  the owner's `Visited` row (coords + place name) and a new idempotent
  `Geographic` row (`LocationType::Geographic = 6`, migration `046`) anchoring
  the place entity's own coordinates, so `find_nearby` resolves places by where
  they are. The overlay worker derives `place_anchor` when a fact's object is a
  `Place` entity.
- **Geocoder:** `GeocodeResult` gained a `short_name` field (the most specific
  locality: city → town → village → hamlet → municipality → county → state →
  region, else the first `display_name` segment). The `Geocoder` trait now
  requires `Debug` (so `ConnectorContext` can derive `Debug`).
- **Refactor:** the location-overlay worker's `OverlayJob::Apply` payload moved
  into a `LocationOverlayApply` struct (fixes a clippy `too_many_arguments`
  lint introduced by the `place_anchor` field; keeps the worker function's
  argument list small).
- **Tests:** geocoder `short_name` unit + Nominatim integration tests; Photos
  `resolve_place` unit tests (mock geocoder place fact, no-geocoder fallback,
  cache hit + miss-then-hit, transient-error-not-cached) + `place_fact` /
  `coords_only_fact` shape tests; integration
  `supervisor_ingests_photo_as_took_photo_at_place_fact`; normalize
  `photos_at_same_place_corroborate_and_anchor_place_coords`; updated
  `lookup_sync_test`, `enum_roundtrip_test`, `migrations_test`,
  `entity_locations_test` for the new enum/count.
- **Docs:** `docs/photos-connector.md` (C2 rewrite), `docs/entity-locations.md`,
  `docs/geocoder.md`, `docs/wiki/photos-connector.md`,
  `docs/wiki/entity-locations.md`, `docs/wiki/geocoding.md`,
  `docs/wiki/what-works-now.md`, `README.md`, `Mimir-Implementation-Context.md`,
  and the `photos.rs` module header.

## [0.80.0] — 2026-07-26

### Connectors (Phase 3 C1 / #195)

- **First concrete connector backend:** the local-filesystem **Photos**
  connector in `mimir-connectors` (feature `photos`). A read-only, push-mode,
  no-network connector that watches a configured directory recursively with
  `notify` (debounced ~2s), extracts EXIF GPS + datetime with `kamadak-exif`
  (JPEG/TIFF/HEIF/PNG/WebP), and emits one `took_photo` fact per photo through
  the shared `normalize_and_insert` pipeline. GPS becomes a `Visited`
  `entity_locations` row for the owner; a per-file mtime/inode incremental
  cursor skips unchanged photos across restarts. Files without GPS still record
  a timestamped fact; files without EXIF fall back to the file mtime. C2
  (#196) will reverse-geocode the coordinates into a place name.
- **Supervisor:** inject the persisted `sync_cursor` into a connector's
  `config_json` as `__cursor` so incremental connectors can read their prior
  progress (the read side that complements `KnowledgeGraph::update_sync_cursor`).
- **Dependencies:** `notify` 8.2, `notify-debouncer-full` 0.7,
  `kamadak-exif` 0.6 — all optional, gated by the `photos` feature.
- **Tests:** 17 unit tests (cursor diffing, EXIF parsing against committed
  fixtures, fact conversion, config) + 9 integration tests (initial scan, the
  live `notify` push watcher, incremental restart skip, the full supervisor →
  knowledge-graph path with a `Visited` GPS location row).
- **Docs:** `docs/photos-connector.md` + `docs/wiki/photos-connector.md`; updated
  `docs/connectors-framework.md`, `docs/wiki/connectors.md`,
  `docs/wiki/what-works-now.md`, `README.md`, and `Mimir-Implementation-Context.md`.

## [0.79.2] — 2026-07-26

### Docs

- **README:** rework the Architecture section. The connectors entry had grown
  into a single ~4000-character run-on paragraph packed with issue references;
  split it into a crate list and a grouped key-subsystems list (knowledge graph,
  learning, retrieval agent, events & reminders, connectors, entity locations)
  so the page is scannable. Also tidy the Acknowledgments spacing.


## [0.79.1] — 2026-07-26

### Review fixes (PR #229)

- **`docs/entity-locations.md`:** add a stable `#proximity-query` anchor so the
  facade-API link resolves to the (Phase-3-suffixed) section heading.
- **`docs/wiki/entity-locations.md`:** drop the obsolete "later release" wording
  for proximity queries (now available in v0.79.0) and document the required `at`
  parameter in the `find_nearby` signature.
- **Migration `045` (`mimir-knowledge`):** correct the NULL-indexing rationale — a
  regular SQLite b-tree index includes NULLs — and switch the composite coordinate
  index to a partial index (`WHERE latitude IS NOT NULL AND longitude IS NOT NULL`)
  so it covers only the geocoded rows `find_nearby` can match and stays smaller.


## [0.79.0] — 2026-07-24

### Entity-locations proximity query (Phase 3 S4 / #194)

- **`KnowledgeGraph::find_nearby(lat, lon, radius_km, at)`** returns every
  `entity_location` within a radius of a point, sorted nearest-first, as
  `Vec<NearbyLocation>` (each entry carries the row and its exact
  great-circle `distance_km`). Closes the query half of #65 (write half in
  #193).
- **Two-stage query:** a coarse SQLite bounding-box pre-filter
  (`latitude`/`longitude BETWEEN ? AND ?`, now backed by a composite
  `idx_entity_locations_coords(latitude, longitude)` index — migration `045`)
  is followed by an exact Haversine post-filter computed in pure Rust that
  drops edge-of-box over-inclusions and sorts the survivors. NULL-coordinate
  (address-only) locations are skipped.
- **Temporal scoping:** `at: Option<DateTime<Utc>>` restricts to locations
  whose `valid_from`/`valid_until` window contains the instant; `None` is a
  pure spatial query over all locations.
- **Pure helpers:** `mimir-knowledge::geo` adds `haversine_km` and
  `bounding_box` (sphere model, mean radius 6371.0088 km, `unsafe`-free,
  allocation-free, unit-tested and benchmarked). No external `geo` crate —
  the formula is small and a heavy dependency for one function would violate
  the minimal-dependency stance.
- Docs: updated `docs/entity-locations.md`, `docs/wiki/entity-locations.md`,
  `docs/wiki/what-works-now.md`, and `README.md`.

## [0.78.2] — 2026-07-24

### Review fixes (PR #225)

- **Shutdown now drains pending location-overlay jobs.** `AppState::shutdown`
  calls `KnowledgeGraph::flush_location_overlays().await` after stopping the
  background scheduler, so queued `entity_locations` upserts complete before
  resources are torn down (previously a shutdown with queued overlays could
  drop the worker before upserting while the source fact remained).
- Docs: removed duplicate `# Changelog` heading (markdownlint MD024); scoped
  the single-worker throughput claim to the default Nominatim backend; fixed
  grammar in the entity-locations wiki.

## [0.78.1] — 2026-07-24

### Entity-locations write path fixes (Phase 3 S3 / #193)

Two correctness/performance fixes to the entity-locations overlay landed in
0.78.0, surfaced by code review:

- **Overlay now uses the inserted fact's temporal bounds.** `process_normalized_fact`
  destructured `valid_from`/`valid_until` once from the extracted fact and never
  updated them, but `handle_correction` mutates `new_fact.valid_from` before the
  insert (a correction scope of `None` becomes `now`, a datetime scope becomes
  that datetime). The location overlay now reads `fact.valid_from`/
  `fact.valid_until` from the *inserted* fact, so the derived `entity_locations`
  row matches its source fact and prior-location supersession fires correctly
  for corrections (e.g. "actually I live at Y now" closes the prior open Home
  instead of inserting a timeless row alongside it).
- **Location overlays are offloaded to a background worker.** `apply_location_overlay`
  was awaited inline inside `normalize_and_insert`'s serial batch loop, so a
  connector batch of location facts was gated on the geocoder's rate limit
  (~1 req/sec for Nominatim). The geocode + upsert is now enqueued to a single
  background worker (FIFO `mpsc` channel) so the ingestion pipeline returns
  immediately and is not stalled by geocoding; the worker processes jobs in
  submission order, preserving move/supersession semantics both within a batch
  and across batches. A single worker loses no geocode throughput versus
  parallelism with the default Nominatim backend, which is already
  rate-limited to ~1 req/sec; a custom or self-hosted `Geocoder` with higher
  throughput could make the single FIFO worker a bottleneck.
  `KnowledgeGraph::flush_location_overlays` awaits every overlay
  enqueued before the call for deterministic shutdown / tests.
- **DRY.** The pool-based supersession upsert is extracted into
  `queries::entity::upsert_location`, shared by the `KnowledgeGraph::upsert_location`
  facade and the background worker.

## [0.78.0] — 2026-07-23

### Entity-locations write path (Phase 3 S3 / #193)

Persist structured locations (address + lat/lng + timezone) for an entity with
temporal validity windows, wired into the shared `normalize_and_insert`
extraction pipeline. Supersedes the write-path half of #65; proximity queries
(`find_nearby`) remain a separate issue (#196).

- **Typed location overlay.** A "where" fact carries an optional
  `NormalizedLocation` (`location_type`, `address`, `latitude`, `longitude`,
  `timezone`) on `NormalizedFact`. After a non-sensitive fact is inserted,
  `apply_location_overlay` derives an `entity_locations` row for the resolved
  subject entity, using the fact's `valid_from`/`valid_until` as the location's
  bounds. Both the conversational `remember` path and connectors fill the same
  field. `NormalizedFact` is `PartialEq`-only now (`f64` coords are not `Eq`).
- **Geocode the missing half.** Via the injected `Geocoder` (stored on
  `KnowledgeGraph` as `Option<Arc<dyn Geocoder>>`): address-only → forward
  geocode to coords; coords-only → reverse geocode to a place name; both
  present → stored as-is. Geocoder errors/no-match are logged and tolerated;
  the pipeline never aborts on a geocode failure.
- **Moves / supersession.** `KnowledgeGraph::upsert_location` closes any
  still-open location of the same `entity_id` + `location_type` whose
  `valid_from` precedes the new `valid_from` (sets `valid_until`), then inserts
  the new row — modelling "home 2020–2023, home 2023–present". Atomic in one
  transaction.
- **Provenance link.** Migration `044` adds a nullable
  `entity_locations.source_fact_id INTEGER REFERENCES facts(id) ON DELETE SET
  NULL`, mirroring `events.fact_id`, so a location row traces to its
  originating fact and survives the fact being forgotten (FK → `NULL`).
- **Daemon wiring.** `AppState::from_config_with_llm` injects the default
  `NominatimGeocoder` into the `KnowledgeGraph` at startup (cheap; no network
  work until a location fact is processed); construction failure disables
  geocoding rather than aborting start.
- **LLM schema.** The `remember` tool schema gained an optional `location`
  object (`location_type` enum Home/Work/Visited/Origin/Current, address,
  latitude, longitude, timezone); `extract.rs` validates it into the overlay.
- **Mock connector.** `MockFactConfig` gained an optional `location` field so
  the connector → location path is exercisable.
- **No `confidence` column.** Locations do not carry their own confidence in
  V1; provenance is via `source_fact_id` and the source fact's confidence.
- **Pending path deferred.** Sensitive "where" facts land as
  `pending_confirmation`; the overlay is not applied until confirmation
  (follow-up).
- **Docs.** `docs/entity-locations.md` (technical), `docs/wiki/entity-locations.md`
  (user-facing), updated `docs/knowledge-graph-schema.md`, `docs/wiki/what-works-now.md`,
  and `README.md`.

## [0.77.0] — 2026-07-23

### Geocoder service (Phase 3 S1 / #191)

A pluggable geocoding abstraction with an OSM Nominatim default backend:
forward geocoding (address / place name → coordinates) and reverse geocoding
(latitude / longitude → place).

- **Trait + types in `mimir-core`.** `Geocoder` (async, object-safe),
  `GeocodeResult` (lat/lon/display_name/country/`country_code`/alternative
  names), and `GeocodeError` live in `mimir-core` so the Location Search tool
  (#98, a `mimir-core` tool) can name the trait without a dependency cycle
  (`mimir-core` cannot depend on `mimir-connectors`). The issue's "lives in
  `mimir-connectors`" wording is treated as referring to the backend, not the
  abstraction.
- **`NominatimGeocoder` backend in `mimir-connectors`.** Issues `GET /search`
  (forward) and `GET /reverse` (reverse) with `format=json&addressdetails=1&
  namedetails=1`; parses Nominatim's string `lat`/`lon` into `f64`. Reuses the
  F12 `RateLimiter` (`RateLimitConfig::nominatim`, ≤ 1 req/s) and
  `retry_with_backoff` for transient 429/502/503/504 + transport failures,
  honouring a server `Retry-After`; daily-quota exhaustion is non-retryable
  (`GeocodeError::RateLimited`).
- **Configurable.** Endpoint (default public instance; self-hosted Nominatim
  supported for heavy use), descriptive `User-Agent` (Nominatim policy),
  optional contact email, `RateLimitConfig`, `max_attempts`, and per-request
  timeout.
- **Result contract.** A successful no-match yields `Ok(None)`; transport /
  decode failures yield `Err(GeocodeError)` (logged, never a panic).
- **Always built** (not behind a feature flag), consistent with the framework
  core + mock connector.
- **Tests.** `mimir-core` unit tests (`MockGeocoder` incl. builder chaining,
  serde round-trip); `wiremock`-backed `mimir-connectors` integration tests
  (forward/reverse parsing, empty → `None`, 429-retry-then-success, persistent
  503 → `Err`, non-retryable 404 no-retry, connection-refused → `Err(Network)`,
  rate-limiter throttling).

This is a library component; daemon wiring lands with the Photos connector
(C2), the entity-locations write path (S3/#65), and the Location Search tool
(#98).

### PR #221 review fixes

- Dropped the unused `reqwest` `query` feature (the backend builds query
  strings by hand) and de-duplicated the `reqwest` dependency comment block.
- `map_rate_err` now maps construction-time `RateLimitConfig` errors to
  `GeocodeError::Backend` instead of `GeocodeError::RateLimited`, reserving
  `RateLimited` for genuine quota-exhaustion at the admission path. Added a
  regression test.

### CodeRabbit review fixes

- Unparseable Nominatim `lat`/`lon` now surface as `GeocodeError::Parse`
  instead of silently becoming `(0.0, 0.0)`; `place_to_result` takes parsed
  `f64` coordinates and a new `parse_coord` helper maps malformed values to
  `Parse`. Added unit + integration regression tests.
- Replaced the hand-rolled query encoder with the vetted `percent-encoding`
  crate (already a transitive dependency via `reqwest`, resolved at 2.3.2);
  space now encodes as `%20` (Nominatim accepts it).
- Removed a broken rustdoc intra-doc link (`GeocodeResult::or_none`) from the
  `mimir-core` geocoder module docs.

## [0.76.0] — 2026-07-23

### Mock connector schema enum source-linking (PR #220 review)

Actions the remaining CodeRabbit review thread on the configurable mock
connector (`mimir-connectors/src/mock.rs`):

- **Source-linked facts schema enums.** The `subject_type`, `object_type`, and
  `recurrence` JSON Schema `enum` lists (and their `default` values) are now
  derived from the serde representation of the canonical enum variant arrays
  instead of being hand-typed strings, so they can no longer silently desync
  from `EntityType` / `RecurrenceType` on a future rename.
- **Canonical variant arrays.** `mimir-knowledge` exposes
  `ENTITY_TYPES` and `RECURRENCE_TYPES` as single sources of truth for any
  caller that must enumerate the variants.
- **Regression guard.** A connector test asserts the schema enum arrays and
  defaults exactly equal the serde-serialised variants, and knowledge tests
  assert the const arrays enumerate every variant in discriminant order.


## [0.75.1] — 2026-07-23

### Mock connector review hardening (PR #220)

Actions CodeRabbit review feedback on the configurable mock connector
(`mimir-connectors/src/mock.rs`):

- **Cancellation-safe sync tracking.** `MockSyncRecorder::enter(options)` now
  returns an RAII `MockSyncGuard` created before the first `.await` of
  `sync()`. Its `Drop` decrements the in-flight counter and records the
  `SyncOptions`, so peak-concurrency tracking stays balanced across injected
  failures, panics, and supervisor shutdown cancellation (push cadence sleep
  and `sync_delay`). Failed/panicked calls are no longer omitted from the
  recorder.
- **Strict `__ctype` validation.** `from_config` rejects non-integer,
  out-of-range, and unknown `__ctype` values with `ConnectorError::Config`
  instead of silently defaulting to Gmail or wrapping via `as i16`. The legacy
  Gmail default is kept only when `__ctype` is absent.
- **Non-zero `batch_size` contract.** `from_config` rejects `batch_size: 0`
  (which would let `sync()` succeed forever while fetching no facts) with
  `ConnectorError::Config`.
- **Fact schema matches the DTO.** The `facts` array item schema is now closed
  (`additionalProperties: false`) and declares the required fields plus the
  `subject_type`/`object_type`/`recurrence` enums, matching `MockFactConfig`.

## [0.75.0] — 2026-07-21

### Phase 3 — configurable mock connector test harness (PR for #190 / F13)

Replaces the placeholder `MockConnector` stub with a configurable,
always-compiled test harness. The mock is driven entirely by its `config_json`
and is the framework's test harness + the T1 sync→extract→insert→query vehicle.

- **Configurable behaviour**: `mode` (`polling`/`push`), `interval_ms`/`jitter_ms`
  cadence, canned `facts` (`MockFactConfig` DTO → `NormalizedFact` with
  `SourceType::Connector`), optional `batch_size` for incremental sync, a static
  `cursor`, configurable `health`/`auth_state`, and `fail_first`/`panic_first`/
  `always_fail` injection. Missing `raw_reference` is auto-generated
  (`mock-<slug>-<index>`) so connector provenance is always satisfied.
  `MockConnector::default()` preserves the legacy no-op identity so existing
  trait tests keep passing.
- **Both modes**: polling paces via the supervisor interval; push self-paces
  via an internal `tokio::time::sleep` inside `sync()` (the supervisor aborts the
  task on shutdown for cancellation). F9 manual triggers remain rejected for
  push connectors.
- **Instance identity**: reads the supervisor-injected `__slug`/`__ctype`/
  `__instance_id` to recover its identity, falling back to the legacy no-op when
  absent.
- **`MockSyncRecorder`**: optional shared observer (`with_recorder`) recording
  the `SyncOptions` each `sync()` receives and the peak in-flight concurrency,
  for F9-style serialization tests. Not part of the config schema or the factory
  path.
- **`config_schema()`**: returns a JSON Schema describing the config surface.
- **DRY consolidation**: the private `TestConnector` in the supervisor lifecycle
  tests was removed; every behavioural lifecycle test now drives the shared
  `MockConnector` (single source of truth for test connectors).
- **T1 vehicle** (`tests/mock_ingestion_e2e.rs`): the real `ConnectorSupervisor`
  + `KnowledgeGraph` ingest a mock's canned facts end-to-end in both polling and
  push modes, asserting KB facts + connector provenance
  (`SourceType::Connector`, `connector_instance_id`, `raw_reference`,
  `ExtractionMethod::StructuredParse`), with no real service.
- **No new dependencies** (in-memory; reuses `tokio`, `serde`, `chrono`).
- **Fix (review #220)**: the `batch_size` slice is now keyed on the
  *successful*-sync counter, not the raw call count, so failed/panicked cycles
  no longer consume a batch window and silently drop facts. Regression tests
  added for the `fail_first` + `batch_size` and `panic_first` + `batch_size`
  combinations.
- **Breaking change** to the public `MockConnector` type (unit struct →
  config-driven struct); acceptable per the project breaking-changes policy
  (only the OpenAI-compatible chat endpoint is stability-sensitive).

Documentation: `docs/mock-connector.md` (technical), `docs/wiki/mock-connector.md`
(user-facing), plus updates to `docs/connectors-framework.md`,
`docs/wiki/connectors.md`, `docs/wiki/what-works-now.md`, and `README.md`.

## [0.74.0] — 2026-07-20

### Phase 3 — rate-limit review follow-ups (PR #219)

Addresses the remaining CodeRabbit review threads on the connector
rate-limiting primitives (issue #189 / F12).

- **Overflow-safe snapshot restore**: `RateLimiter::with_quota_state` now
  validates a restored `QuotaSnapshot` with `DateTime::checked_add_signed`
  before constructing quota state, returning the new
  `RateLimitError::InvalidSnapshot` when `window_start + window` would
  overflow `DateTime<Utc>`. A crafted, `serde`-deserialisable snapshot near
  `DateTime::<Utc>::MAX_UTC` can no longer panic inside `is_exhausted` /
  `check_and_increment`.
- **Monotonic persistence protocol**: `QuotaSnapshot` gains a `version: u64`
  field (with `#[serde(default)]`) that increases on every successful `acquire`
  and is carried across reconstruction, so a persistence layer can use it as a
  compare-and-swap guard and never regress a window's count via delayed,
  out-of-order writes. `docs/connector-rate-limiting.md` documents the
  persist-before-dispatch and never-regress protocol.
- Tests added for `MAX_UTC` snapshot rejection, monotonic `version` across
  acquires and reconstruction, and backward-compatible deserialisation of
  pre-`version` snapshots.

## [0.73.0] — 2026-07-17

### Phase 3 — rate-limit review fixes (PR #219)

Addresses CodeRabbit review feedback on the connector rate-limiting primitives
(issue #189 / F12).

- **New public API** for daily-quota persistence across restarts:
  `QuotaSnapshot`, `RateLimiter::with_quota_state`, and
  `RateLimiter::quota_snapshot`. A reconstructed limiter resumes the saved
  rolling 24h window instead of resetting the allowance to zero, so a daemon or
  connector relaunch cannot silently bypass a provider's hard 24-hour quota.
- **Fail-fast quota exhaustion** in `RateLimiter::acquire`: a known-exhausted
  daily quota is now reported before awaiting the token bucket, so a low-rate
  limiter no longer parks a task for the full replenish interval (potentially
  hours) before returning `QuotaExhausted`. The authoritative increment still
  runs after token admission.
- **Bounded jittered delay**: the retry delay is now clamped *after* jitter is
  applied, so a `Retry-After` at the strategy cap can no longer become
  `cap + jitter` and breach the bounded-delay contract (`retry_delay_with_jitter`).
- **Saturating duration arithmetic**: the `Linear` backoff and jitter paths use
  `Duration::saturating_add`, preventing overflow panics on
  config-derived values near `Duration::MAX` before the `max` clamp.
- **Documentation** corrected to describe the primitives as available
  infrastructure for future connector adoption, not as already wired into every
  connector's outbound calls.
- Tests added for snapshot round-trip/restore, fail-fast exhaustion,
  clamp-after-jitter, and near-`Duration::MAX` saturation; the daily-quota
  `resets_at` assertion now verifies an ~24h window rather than merely
  `> before`.

## [0.72.0] — 2026-07-17

### Phase 3 — connector rate limiting & retry (issue #189 / F12)

Shared rate-limiting + retry/backoff primitives for network connectors, in the
new `mimir_connectors::rate_limit` module. The primitives are available
infrastructure now; connectors will route their outbound HTTP/IMAP/CalDAV API
calls through one per-instance `RateLimiter` for uniform throttling, daily-quota
enforcement, and 429/503 retry as their backends land in later Phase 3 issues.

- `RateLimitConfig { requests_per_second, burst_size, daily_quota,
  backoff_strategy }` — `serde`-serialisable (human-readable durations via
  `humantime`) so it embeds in each connector's `config_json`; a
  `RateLimitConfig::nominatim()` preset enforces the OSM Nominatim ≤ 1 req/s
  usage policy.
- `RateLimiter` — token bucket backed by `governor` (a vetted, `unsafe`-free
  GCRA implementation) for `requests_per_second` + `burst_size`, with an
  optional rolling 24h daily quota. Quota exhaustion returns a non-blocking
  `RateLimitError::QuotaExhausted { resets_at }` so the `ConnectorSupervisor`
  can pause the cycle gracefully instead of parking a task for up to 24h.
- `BackoffStrategy` — exponential / linear / fixed, each with a jitter budget.
- `retry_with_backoff` — generic retry helper for transient failures, with a
  `Retryable` trait and `RetryHint::from_status` classifying
  `{429, 502, 503, 504}` (matching the `LlmClient` transient set) and
  honouring a server-supplied `Retry-After`, clamped to the strategy's `max` cap
  (or a 5-minute default for `Fixed`) so an unreasonable hint cannot stall a
  connector task.
- Connector **LLM** calls are exempt (decision D′): they route through the
  shared `LlmWorkerPool` system queue and are not wrapped by this limiter.

New dependencies (version-checked on crates.io): `governor` 0.10, `rand` 0.9
(pinned to the line `governor` already pulls in transitively), `humantime`
2.4. No `sqlx`; no `unsafe`. Unit + integration tests cover throttling,
quota exhaustion/reset, backoff progression, retry success/exhaustion/terminal,
`Retry-After` honouring + clamping, config serde, and presets.

## [0.71.1] — 2026-07-17

### Docs

- Corrected the `FileSecretStore` atomic-write documentation in
  `docs/connector-secret-store.md` to describe the actual temporary-file
  naming pattern (`<slug>.json.tmp.<pid>.<counter>`, using the process id and
  a per-process monotonic counter) rather than the placeholder
  `<slug>.json.tmp`, so cleanup and monitoring logic is not misled. Addresses
  PR #218 review feedback.

## [0.71.0] — 2026-07-17

### Phase 3 — connector secret store (issue #187 / F10)

A single `SecretStore` trait now backs every connector auth kind. One
`SecretBundle` enum covers OAuth 2.0, API tokens, and app passwords, and the
V1 default `FileSecretStore` persists one JSON file per connector instance
under `~/.local/share/mimir/secrets/<slug>.json`, file mode `0600`, parent
directory `0700`, plaintext at rest. Loads *fail closed*: a secret file or
directory with any group/other permission bits set is refused
(`SecretError::InsecurePermissions`) rather than read. Writes are atomic
(temp file + rename) so a crash cannot truncate a secret. Slugs are validated
against `[A-Za-z0-9_-]{1,128}` before touching the filesystem, blocking
path-traversal. An `InMemorySecretStore` is included as a test/helper backend.

At-rest encryption is intentionally deferred (consistent with the plaintext
LLM API key in `config.toml` and the home-directory trust boundary); a
`keyring`-backed store is tracked separately as #188. The end-to-end
`connector remove` secret wipe is the consumer's responsibility (server/CLI
routes in #202/#204/#203); this issue delivers the `delete(slug)` capability.

- **New public API (mimir-connectors):** `SecretStore`, `SecretBundle`,
  `FileSecretStore`, `InMemorySecretStore`, `SecretError`.
- **New public API (mimir-core paths):** `secrets_dir()`, `secrets_file(slug)`.
- **No new dependencies** (uses existing `serde`/`serde_json`/`chrono`/
  `async_trait`/`thiserror`/`tracing`; permission enforcement uses the
  std `std::os::unix::fs::PermissionsExt` safe API).
- **Design note:** `SecretBundle` uses struct variants (`ApiToken { token }`,
  `AppPassword { password }`) rather than newtype variants so serde's
  internally-tagged `kind` representation works; the on-disk JSON is
  self-describing. `OAuth.refresh_token` and `OAuth.expires_at` are `Option`
  since not all grants issue a refresh token or return an expiry.
- Framework core (not feature-gated); `--no-default-features` still compiles
  the secret store alongside the rest of the framework + mock.

## [0.70.0] — 2026-07-14

## [0.70.0] — 2026-07-14

### Phase 3 — manual sync triggering (issue #186 / F9)

`ConnectorSupervisor::trigger_sync(id, SyncOptions)` (and a slug-based
`trigger_sync_by_slug`) wakes a connector's runner from its polling-interval
wait so a sync runs immediately with caller-supplied options — `--full`
forces a non-incremental pass (cursor ignored/reset) and `since` is a
relative time-window hint. A one-permit `tokio::sync::Semaphore` per
connector serialises concurrent callers (overlapping triggers queue rather
than launching parallel cycles), and a per-connector request channel carries
the options and returns the cycle's `TriggerOutcome` (`Ok { fetched,
new_cursor }`, `AuthExpired`, or `Failed`). Triggering a connector that is
not running (`Paused`/`Error`/`Setup` or exited) returns
`TriggerError::NotRunning`; push-mode connectors (no polling interval to
preempt) return `TriggerError::PushUnsupported` — push manual sync is
deferred to a later Phase 3 issue.

- **New public API:** `ConnectorSupervisor::trigger_sync`,
  `ConnectorSupervisor::trigger_sync_by_slug`, `TriggerOutcome`,
  `TriggerError`, and `SyncOptions::Default` (incremental, no window).
- **Runner loop rework:** the post-cycle wait is now a `select!` between the
  polling interval, a manual trigger, and shutdown; a manual trigger preempts
  the interval. Backoff after a failed cycle is likewise preemptable by a
  trigger. `run_cycle` takes `SyncOptions`; `CycleOutcome::Ok` now carries the
  `SyncOutcome` so a triggered cycle can report stats to the caller.
- **No new dependencies** (`tokio` `sync` feature already enabled).
- This is a library component in `mimir-connectors` with integration tests
  against a configurable in-memory mock; daemon `AppState` wiring and the
  `mimir connector sync …` CLI land in later Phase 3 issues (A1–A3).

## [0.69.2] — 2026-07-13

### Review fixes for PR #216 (CodeRabbit)

- **Docs:** clarified the shutdown integration boundary in
  `docs/wiki/connectors.md` and `docs/wiki/what-works-now.md` — the supervisor
  supports the shared `watch` shutdown channel, but `mimir stop` does not yet
  drive it (daemon/CLI wiring is deferred to later Phase 3 issues).
- **`mimir-connectors`:** de-duplicated the `CycleResult::Err` and
  `CycleResult::Panic` failure handling into a shared `record_failure` async
  helper so the circuit-breaker / backoff policy cannot drift between the
  sync-error and panic paths.
- **Tests:** `none_cursor_preserves_existing_sync_cursor` now captures the
  seeded `last_sync_at` and waits for it to advance (not merely exist),
  ensuring the poll observes a completed `None`-cursor cycle rather than the
  pre-existing row state.

## [0.69.1] — 2026-07-13

### Bugfix — None sync cursor no longer wipes persisted progress token

`run_cycle` previously passed `SyncOutcome::new_cursor` straight into
`KnowledgeGraph::update_sync_cursor`, whose `None`-clears contract wiped the
persisted `sync_cursor` whenever a connector returned `new_cursor: None`
(documented as "unchanged"). The next incremental sync then re-fetched from
the beginning, defeating the "no re-fetch after `mimir stop`" guarantee.

- **Fix:** `run_cycle` now branches — `Some(cursor)` advances (or clears) the
  cursor via `update_sync_cursor`; `None` stamps `last_sync_at` only via the
  new `KnowledgeGraph::touch_last_sync`, preserving the progress token.
- **New KG method:** `touch_last_sync(id)` — stamps `last_sync_at` and
  `updated_at` without rewriting `sync_cursor`.

## [0.69.0] — 2026-07-13

### Phase 3 — ConnectorSupervisor supervised lifecycle (issue #185 / F8)

`ConnectorSupervisor` owns one supervised background task per connector whose
lifecycle status is `Active`, centralising everything needed to keep a
connector running safely. It is the caller that runs the two-step ingestion
model end to end: `health` → `sync` → `extract` → `normalize_and_insert`, then
`update_sync_cursor`. All status / auth / cursor writes go through the
`KnowledgeGraph` facade — the crate stays `sqlx`-free. No concrete backends
sync yet (this is a library component; daemon wiring lands in A1).

- **`SupervisorConfig`** (`max_failures`, `base_backoff`, `max_backoff`):
  injected at construction (no environment mutation, per the safety policy).
  Backoff is deterministic in V1 (`base_backoff * 2^(n-1)`, capped at
  `max_backoff`); randomised jitter / rate-limit primitives belong to F12.
- **`ConnectorSupervisor`**:
  - `restore()` — loads the `connectors` table and spawns a runner for every
    `status == Active` row. `Paused` / `Error` / `Setup` rows are left down.
    Rows with no registered `(type, backend)` factory or invalid `config_json`
    are logged and skipped (one bad connector never aborts startup).
  - `shutdown()` — aborts and joins all runner handles (defensive fallback;
    the shared `watch` channel normally drains them first).
  - `running_count()` / `is_running(id)` — observability for tests/wiring.
- **Per-connector runner loop.** Initial `authenticate()` handshake (a failed
  handshake pauses and exits). Then each cycle runs in an **isolated sub-task**
  (`tokio::spawn`) so a connector panic surfaces as `JoinError::is_panic`
  instead of unwinding the runner. The cycle and the shared shutdown signal race
  in a `tokio::select!`; shutdown aborts the in-flight cycle via its
  `AbortHandle`.
- **Lifecycle mapping.** Success resets failures, persists the cursor, clears
  `last_error`. Error/panic increments failures (status stays `Active` +
  recorded `last_error`), applies exponential backoff, and after `max_failures`
  consecutive failures moves the connector to `Error` (circuit breaker; stops
  auto-restart, manual `resume` required). `health() == AuthExpired` sets
  `auth_state = Expired` + `status = Paused` and exits (not auto-restarted).
- **Graceful shutdown + cursor persistence.** The supervisor observes the same
  shared `watch::Receiver<bool>` shutdown channel the daemon uses for OS signals
  and `/stop`, so `mimir stop` drains every runner. The cursor is persisted
  after every successful `sync`, so it always reflects the last completed sync;
  `mimir stop` mid-cycle aborts the in-flight cycle (no cursor advance) and the
  next restart resumes from the last persisted cursor.
- **Instance identity injection.** `restore` augments each row's `config_json`
  with `__slug`, `__ctype`, and `__instance_id` before passing it to the
  factory — the V1 mechanism for giving a connector instance its row identity
  through the minimal `create(config)` signature (the LLM/SecretStore
  construction context is deferred to F10 / the first real backend).
- **`yield-on-user-activity`** deferred for V1 (`last_user_activity` is not
  consulted yet).
- **`SupervisorError`** (`Knowledge`, `Connector`, `Json`): thiserror enum.
- **Dependencies.** `tokio` promoted from dev-only to a real dependency of
  `mimir-connectors` (`rt`, `sync`, `time`, `macros`); `tracing` added for
  structured logging. No new external crates (tokio is already pinned to `1`
  workspace-wide; patterns verified via Context7).
- **Tests.** New `tests/supervisor_lifecycle.rs` (7 integration tests against a
  real in-memory knowledge graph and a configurable test-local connector mock):
  startup restore (Active only), cursor persistence on shutdown, transient
  failures → backoff → recovery, circuit breaker, auth-expiry pausing, panic
  recovery, and push-mode in-flight cancellation. The shared `MockConnector`
  is untouched (F13 owns the real harness).


## [0.68.0] — 2026-07-08

### Phase 3 — ConnectorRegistry + multi-backend factory dispatch (issue #184 / F7)

`ConnectorRegistry` now maps `(connector_type, backend)` to a
`ConnectorFactory`, enabling the multi-backend architecture: a connector
*type* (Email/Calendar/Photos) is the provenance/reliability axis; a *backend*
(IMAP, CalDAV, local-FS, …) is the provider implementation chosen per instance
and stored as the `backend` column on `connectors` (F2). New backends register
a new factory — no schema change. No concrete backends sync yet.

- **`ConnectorFactory` trait** (`Send + Sync`, object-safe): `create(config:
  serde_json::Value) -> Result<Arc<dyn Connector>, ConnectorError>`.
  Construction is synchronous and cheap; network/auth happen later via
  `Connector::authenticate` / `sync`. The V1 factory takes only `config`;
  decision D′ (`Arc<dyn LlmBackend>` at construction) and F10 (`SecretStore`)
  will extend the signature when F8/F10 land (acceptable internal-API break).
- **`ConnectorRegistry`** (`RwLock<HashMap<(ConnectorType, String),
  Arc<dyn ConnectorFactory>>>`, `&self` registration matching `ToolRegistry`):
  `register`/`register_arc`, `is_registered`, `factory`, `backends_for`,
  `registered_types`, `create`, plus `len`/`is_empty`. Duplicate
  `(type, backend)` registration fails loud with `BackendAlreadyRegistered`;
  unknown-pair `create` returns `BackendNotFound`.
- **`FnConnectorFactory`** — closure-backed factory for simple backends/tests.
- **`MockConnectorFactory`** — always-compiled factory producing
  `MockConnector`s, keeping the registry exercisable under every feature
  combination (including `--no-default-features`).
- **`ConnectorError`** — new variants `BackendNotFound` and
  `BackendAlreadyRegistered`.
- **`mimir-knowledge`** — `ConnectorType` now derives `Hash` so it can key the
  registry's `HashMap`.
- **Reliability stays per-type:** confidence remains
  `confidence::initial(SourceType::Connector, connector_type)`, keyed on the
  type axis only; the registry never branches reliability on `backend`.

### Review fixes (PR #215)

- **deps:** Removed redundant `async-trait` and `chrono` re-declarations from
  `mimir-connectors` `[dev-dependencies]` (both are already regular
  dependencies and thus available to integration tests) — DRY.
- **registry:** Poisoned-`RwLock` handling is now consistent across every
  accessor: all read/write acquisitions go through private `read`/`write`
  helpers that `.expect` on poison, matching the `ToolRegistry` convention.
  Previously the write-side methods surfaced poison as
  `ConnectorError::Other` while read-side methods silently returned
  empty/false/`None`, which could report contradictory state after a panic.
## [0.67.1] — 2026-07-08

### Review fixes (PR #214)

- **docs:** Corrected the `mimir-connectors` framework status summary so the
  mock harness is no longer described as a stub (it is now implemented and
  documented), keeping the phase status consistent with the `MockConnector`
  section.
- **tests:** The `trait_is_object_safe` test now exercises the default
  `act()` implementation through `Arc<dyn Connector>`, confirming it
  dispatches correctly through the trait object and returns
  `ConnectorError::UnsupportedAction`.

## [0.67.0] — 2026-07-08

### Phase 3 — Connector trait + data types (issue #183 / F6)

The runtime `Connector` trait and its supporting data types are now defined in
`mimir-connectors`, replacing the F1 identity-only stub. This is the contract
every service-ingestion worker implements; no concrete backends yet.

- **`Connector` trait** (`#[async_trait]`, `Send + Sync`, object-safe as
  `dyn Connector`): `id()` (instance slug), `name()`, `connector_type() ->
  ConnectorType`, `mode() -> ConnectorMode`, `config_schema() ->
  serde_json::Value`, `authenticate() -> ConnectorAuthState`, `health() ->
  HealthStatus`, `sync(SyncOptions) -> SyncOutcome`, `extract() ->
  Vec<NormalizedFact>`, optional `act(ConnectorAction) -> ActionResult`
  (default impl returns `UnsupportedAction`), and `forget()`.
- **Ingestion model (locked):** two-step and DB-free. `sync()` fetches raw
  items into a connector-internal buffer; `extract()` drains them into typed
  `NormalizedFact`s (entity *types* set, entity *ids* unresolved). The
  supervisor (F8) builds the `Provenance` and calls
  `mimir_knowledge::normalize::normalize_and_insert` to resolve entities, score
  confidence, gate sensitivity, and insert. The trait takes no
  `&KnowledgeGraph`, so connectors stay `sqlx`-free and unit-testable without a
  live knowledge graph.
- **New data types:** `ConnectorMode { Polling{interval,jitter} | Push }`,
  `SyncOptions { full, since }`, `SyncOutcome { fetched, new_cursor,
  fetched_at }`, `HealthStatus { Online | Offline | Degraded | AuthExpired |
  NotConfigured }`, `ConnectorAction`, `ActionResult`, and a `thiserror`-based
  `ConnectorError`.
- **`HealthStatus` is a transient runtime probe**, deliberately renamed to
  disambiguate from the persisted `ConnectorStatus` / `ConnectorAuthState`
  enums; the supervisor maps probe outcomes onto the persisted lifecycle
  columns.
- **Reuse (DRY):** `ConnectorType` / `ConnectorAuthState` and `NormalizedFact`
  are consumed directly from `mimir-knowledge`; no parallel `ExtractedFact` /
  `RawEvent` types are introduced.
- `MockConnector` now satisfies the full trait so the always-compiled mock path
  stays valid under every feature combination. 13 new behavioural tests cover
  the trait surface, object safety, the polling/push distinction, and the
  renamed health variants.

Dependencies added to `mimir-connectors` (version-checked on crates.io):
`async-trait 0.1`, `serde 1.0`, `serde_json 1.0`, `chrono 0.4`, `thiserror 2.0`
(+ `tokio 1` dev-dependency). No `sqlx`.

## [0.66.0] — 2026-07-08

### Phase 3 — Full entity-resolution chain (issue #182 / F5)

`resolve_entity` (`mimir-knowledge::normalize`) now runs the full resolution
chain shared by chat extraction and connectors: exact name → exact alias →
FTS5 fuzzy → create new, with two correctness policies layered on top of the
existing `get_by_name` search:

- **Strict same-type filtering.** A new `get_by_name_typed` lookup restricts
  candidates to the requested `EntityType`, so a cross-type token-overlap match
  (e.g. "Apple" resolved as a `Concept` vs "Apple Inc" the `Organization`) is
  dropped and a new entity is created instead of a wrong merge. The untyped
  `get_by_name` remains the general-purpose search surface.
- **Fuzzy resolution gate.** A pure `pick_resolution` policy resolves
  exact-name and exact-alias hits unconditionally, but accepts a fuzzy hit only
  when its normalised score is ≥ `FUZZY_RESOLVE_THRESHOLD` (`0.9`); weaker
  fuzzy matches fall through to create-new. Alias creation is not auto-learned
  from fuzzy matches — it stays explicit via `preferred_name`.

Tests: 9 unit tests for the decision policy (threshold boundary, alias/exact
precedence) and 6 integration tests through `normalize_and_insert` (alias-,
fuzzy-, exact-, create-on-miss, and cross-type-create paths). No regression in
the chat extraction E2E suite.

## [0.65.0] — 2026-07-08

### Phase 3 — Shared normalize/insert boundary (issue #181 / F4)

Extract the resolve → confidence → sensitivity-gate → insert orchestration from
the conversational `remember` path into a reusable
`normalize_and_insert(kg, Vec<NormalizedFact>, Provenance) -> ExtractionOutcome`
boundary in `mimir-knowledge::normalize`. Both chat learning and (future)
service connectors now funnel through one deterministic Rust pipeline.

- **New public types** in `mimir-knowledge::normalize`:
  - `NormalizedFact` — provenance-annotated, per-fact content with typed entity
    types, parsed temporal bounds, typed `RecurrenceType`, validated category
    ids, the sensitivity flag, an optional correction scope, and the per-fact
    `raw_reference`. `source_type` is per-fact because a chat batch may mix
    `Explicit`/`Casual` facts; connectors set `Connector`.
  - `Provenance` — batch-level origin: the connector instance id + type (for
    connector syncs) and the `extraction_method` (`LlmExtraction` for chat,
    `StructuredParse` for structurally-parsed connector items). Constructors
    `Provenance::chat` and `Provenance::connector`.
  - `ExtractionOutcome` / `PendingFact` move to `normalize` and are re-exported
    from `mimir_knowledge::extract` for existing callers.
- **Confidence** is `confidence::initial(source_type, connector_type)` — the
  per-source-type / per-connector reliability score with **no extraction-method
  discount**. Corroboration, supersession, and inference are inherited for free
  from `insert_fact_in_tx`.
- **Sensitivity** uses the same Rust `AND`-gate as conversational facts:
  connector facts the producer flags sensitive land as `pending_confirmation`
  and surface via `kb audit`.
- **Conversational refactor:** `extract.rs` keeps the LLM-call half (tool
  schema, prompts, output parsing) plus an `extracted_to_normalized` adapter
  that canonicalises predicates, splits list objects, and parses LLM string
  fields into typed `NormalizedFact`s. `process_remember_output`,
  `extract_facts`, and `extract_facts_with_context` route through
  `normalize_and_insert`; chat behaviour is unchanged.
- **Tests:** new `mimir-knowledge/tests/normalize_test.rs` covers a
  connector-produced `NormalizedFact` insert, the cross-connector corroboration
  acceptance criterion (Gmail flight + Calendar event → one fact, two sources,
  confidence boosted to the 0.95 cap), chat provenance, and the connector
  provenance gate. All existing extraction/optimization/confirmation tests pass
  unchanged.

### Tests — pre-existing `mimir-server` harness failures fixed

Six `mimir-server` lib tests that failed identically on `main` (verified in a
detached worktree) were stale test-harness bugs, not production defects:

- **`insert_pending_fact` helper** (5 `test_kb_*` tests): the helper built a
  sensitive allergy fact with `is_sensitive: true` but **no catalogue category**.
  After the #142 sensitivity `AND`-gate landed, Rust correctly narrows such a
  fact to non-sensitive (no sensitive category and no sensitive keyword in
  `"peanuts"`), so it never reached `pending_confirmation` and the helper
  panicked indexing into an empty result. Fixed by assigning category 230
  (Allergies & Intolerances), mirroring `extract.rs`'s `sensitive_allergy_fact`
  helper.
- **`test_non_incognito_allows_remember_tool_and_persists_fact_stream`** and the
  paired `test_incognito_..._stream` (which was a false pass): both hit
  `/chat/stream` but queued responses via the blocking-path mock API
  (`push_chat_message`/`push_chat`) instead of the stream-path API
  (`push_stream`/`StreamItem`). The stream therefore errored out on an empty
  queue before executing the `remember` tool. Fixed by queueing
  `StreamItem::ToolCalls` + a follow-up `StreamItem::Text`, and (for the
  non-incognito case) draining the SSE body so the spawned stream task completes
  fact persistence before the assertion. The incognito test now exercises the
  incognito write-guard for the right reason instead of passing by accident.

Production code was unchanged; only test harnesses were corrected.

### Review fix — scope-less correction regression

The boundary originally gated `handle_correction` on `correction_scope.is_some()`,
which silently dropped the defensive temporal-correction-at-`now` behaviour: a
conversational `Correction` fact with no `correction_scope` (which the LLM may
emit despite being told to set one) was treated as an ordinary `Explicit` fact
and never superseded its open-ended predecessor. Fixed by carrying an
`is_correction: bool` on `NormalizedFact` (set by the chat adapter from the LLM
`Correction` classification; connectors always leave it `false`) and gating on
that signal instead, so the `handle_correction` `None` arm is reachable again. A
regression test (`test_correction_no_scope_defaults_to_temporal_at_now`) covers
the path.

### Review fix — sensitive facts dropped catalogue categories

`insert_sensitive_fact` (the pending-confirmation insert path) wrote the fact
and source but skipped the `fact_categories` junction writes that the normal
insert path performs, so sensitive facts lost their catalogue category links.
Category-based reads and downstream memory/sensitivity logic could therefore
miss them. Fixed by persisting `new_fact.category_ids` in the same transaction
via `INSERT OR IGNORE INTO fact_categories`, mirroring `insert_fact_internal`.
A regression test (`sensitive_fact_persists_its_catalogue_categories`) covers
the path.

### Review fix — markdown lint hygiene

- `docs/wiki/what-works-now.md`: replace the blank blockquote separator (MD028,
  no-blanks-blockquote) between the release-summary and corroboration blocks
  with a plain blank line so they render as separate blockquotes.
- `docs/fact-extraction-pipeline.md`: add a blank line after the new
  `### Scope-less Correction (`None`)` heading (MD022, blanks-around-headings).

## [0.64.0] — 2026-07-07

### Phase 3 — Sources provenance FK migration (issue #180 / F3)

Migrate `sources.connector_id TEXT` to
`connector_instance_id INTEGER REFERENCES connectors(id)`, so every fact's
connector provenance points at a registered connector instance instead of a
free-form string label. `connector_type_id` is retained (denormalised) so the
confidence model can read the connector kind without a join.

- **Migration `043_sources_connector_instance_fk.sql`** rebuilds the `sources`
  table (SQLite cannot change a column type in place). It is lossless for
  existing DBs: legacy `sources.connector_id` values were already limited to
  `NULL` or `''` (the insert paths differ — `queries/source.rs` normalised a
  missing connector to `''`, `queries/fact.rs` bound `NULL`), and both map to
  `connector_instance_id IS NULL`. The NULL-aware unique index is
  rebuilt as `(fact_id, source_type_id, COALESCE(connector_instance_id, 0),
  COALESCE(raw_reference, ''))` (`0` is a safe sentinel since autoincrement ids
  start at `1`), plus a new `idx_sources_instance` index for item-count queries.
- **Rust model:** `Source.connector_instance_id: Option<i32>` and
  `NewFact.connector_instance_id: Option<i32>` replace the old `Option<String>`
  labels. Every insert site (`extract.rs`, the corroboration + `insert_fact_in_tx`
  paths in `queries/fact.rs`, `queries/trash.rs` restore, `optimization/mod.rs`
  dedup-merge), the `SourceInput`/`AddSourceRequest` facade, and the public
  `SourceRow` wire type (`connector_instance_id: Option<i32>`) were updated.
- **Validation gate:** `insert_fact` now keys connector provenance off
  `connector_instance_id` rather than `connector_type`. When set it requires
  `raw_reference` and `extraction_method`, resolves the instance, and enforces
  consistency — a supplied `connector_type` must match the instance's registered
  `connector_type_id` (else `KnowledgeError::Validation`), or it is derived from
  the instance when omitted. An unregistered instance id is rejected. This
  provenance validation always runs when `connector_instance_id` is set,
  independent of whether `confidence` is supplied explicitly, so an explicit
  confidence can no longer bypass the `raw_reference`/`extraction_method`
  requirement or the `connector_type` consistency check.
- **Forget filter:** `forget --source <slug>` now matches `connectors.slug` via
  subquery (plus the existing `source_types.name` arm), since the column is no
  longer a free-form string.
- **Item counts** are now derivable via
  `SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?`, closing the
  acceptance deferred from #179.

## [0.63.0] — 2026-07-07

### Phase 3 — Connector instance registry (issue #179 / F2)

The `connectors` instance-registry table and its `KnowledgeGraph` facade.
Each row is a single configured connector instance (one Gmail account, one
CalDAV calendar, …); connector backends persist their sync cursor, auth
state, and health here so they survive daemon restarts. This unblocks the
supervisor (F8) and per-connector backends (C1–C7).

- **Migration `042_create_connectors.sql`** adds the `connectors` table plus
  two lookup tables, `connector_statuses` (`Setup`, `Active`, `Paused`,
  `Error`) and `connector_auth_states` (`Unauthenticated`, `Authenticated`,
  `Expired`). Integer PKs and `_id` foreign keys follow project convention;
  `slug` is a unique human label. The `sources.connector_instance_id`
  provenance FK and item-count query are deferred to F3.
- **Typed enums:** `ConnectorStatus` and `ConnectorAuthState`
  (`#[repr(i16)]`, `sqlx::Type`, `TryFrom<i16>`) in `models/enums.rs`, with
  stability and serde round-trip tests. This deliberately deviates from the
  issue's `TEXT` schema to match the existing `EventStatus`/`FactStatus`
  pattern and the project's "smallest data type" rule.
- **Model + queries:** `models/connector.rs` (`Connector` row + typed
  accessors + `UpsertConnectorInput`) and `queries/connector.rs`. The
  `Connector` row stores lookup ids as raw `i16` with typed accessors,
  mirroring the `Event` overlay model.
- **Facade methods** on `KnowledgeGraph`: `list_connectors`,
  `get_connector_by_slug`, `get_connector`, `upsert_connector`,
  `update_sync_cursor`, `set_connector_status`, `set_auth_state`. Upsert is
  keyed on `slug`; `slug` and `connector_type` are immutable identity. On
  conflict it updates the mutable config surface (`backend`, `display_name`,
  `config_json`, `status`, `auth_state`) and preserves sync-progress fields
  (`sync_cursor`, `last_sync_at`, `last_error`); reusing a slug with a
  different `ConnectorType` returns `KnowledgeError::ConnectorTypeMismatch`
  rather than silently rewriting the instance's kind. `set_connector_status`
  takes an `Option<Option<String>>` `error` parameter (leave / clear / set
  `last_error`). New `KnowledgeError::ConnectorNotFound` (unknown id) and
  `ConnectorTypeMismatch` variants; the typed `ConnectorType` input makes the
  `connector_types` FK guaranteed valid.
- **Tests:** `tests/connectors_test.rs` covers defaults, slug/id/list lookup,
  upsert update-vs-preserve, connector-type mismatch rejection, sync cursor,
  status set/clear/leave, auth state, duplicate-slug upsert, and missing-id
  errors; `tests/migrations_test.rs` asserts the new tables exist and are
  seeded.
- **Docs:** updated `docs/connectors-framework.md`, `docs/wiki/connectors.md`,
  `README.md`, `Mimir-Implementation-Context.md`, and
  `docs/wiki/what-works-now.md`.

## [0.62.1] — 2026-07-02

### Bugfix — shutdown trigger attribution logging

The daemon logged only `Server shut down gracefully.` for **every** shutdown
path, so the *cause* of a stop was unobservable from the journal. An
unexplained stop on 2026-06-30 (systemd recorded no `Stopping`/`Stopped`
lifecycle line, proving the trigger originated inside the process) could
not be attributed from logs.

- **`ShutdownSource` enum** (`mimir-server/src/lib.rs`) classifies the origin
  of a shutdown: `StopEndpoint(SocketAddr)`, `Terminate` (SIGTERM), or
  `Interrupt` (Ctrl-C / SIGINT). Each trigger path now logs
  `ShutdownSource::attribution()` *before* firing the shared `shutdown_tx`
  watch trigger, e.g. `Shutdown requested via /stop endpoint from 127.0.0.1:45678.`
- **`/stop` handler** (`mimir-server/src/routes/stop.rs`) now captures the
  requesting peer via an axum `ConnectInfo<SocketAddr>` extractor and logs it
  (loopback-guaranteed by the existing `require_loopback` middleware).
- **OS-signal listener** (`spawn_os_signal_shutdown`) now distinguishes which
  signal fired (SIGTERM vs Ctrl-C) and logs the matching attribution.
- **Untriggered exits are no longer mislabelled.** `serve_with_bounded_drain`
  previously logged `Server shut down gracefully.` even when the server future
  resolved on its own with no shutdown trigger (Phase 1). This is now reported
  at `warn!` level as `Server future resolved without a shutdown trigger; exiting.`
  via the pure `server_exit_message(triggered: bool)` helper, so a non-graceful
  exit can no longer masquerade as graceful.
- **No public API change.** `ShutdownSource` and `server_exit_message` are
  `pub(crate)`; the `/stop` route behaviour is unchanged.
- **Tests:** `test_shutdown_source_attribution_messages`,
  `test_server_exit_message_distinguishes_untriggered_exit`, and
  `test_stop_handler_fires_shutdown_trigger` (TDD).
- **Docs:** updated `docs/shutdown.md` and `docs/wiki/daemon-shutdown.md`.


## [0.62.0] — 2026-07-02

### Phase 3 — Connectors framework scaffold (issue #178 / F1)

- **New crate:** `mimir-connectors` — the service ingestion framework for
  Mimir. Connectors are background sync workers that fetch external data
  (email, calendar, photos) and normalize it into knowledge-graph facts through
  the existing fact pipeline. Wired into the workspace `members` and added as a
  dependency of `mimir-server`.
- **DB-access boundary:** the crate depends on `mimir-core` and `mimir-knowledge`
  only and never declares a direct `sqlx` dependency; all persistence goes
  through the `KnowledgeGraph` facade.
- **Feature flags:** `default = ["photos","calendar","gmail"]`; the framework
  core and mock connector are always built, so
  `cargo build -p mimir-connectors --no-default-features` still compiles a
  working framework + mock harness. The flags gate no code yet — backends land
  in later Phase 3 issues (C1–C7).
- **Stubs:** minimal `Connector` trait, `ConnectorRegistry`, and `MockConnector`
  placeholders that compile and are object-safe. F6/F7/F13 own the real
  implementations.
- **Safety:** `#![deny(unsafe_code)]` enforced at the crate root.
- **Tests:** scaffolding smoke test asserting the registry constructs empty and
  the mock connector implements the trait (passes under all feature
  combinations).
- **Docs:** added `docs/connectors-framework.md` (technical) and
  `docs/wiki/connectors.md` (user-facing); updated `README.md`,
  `docs/workspace.md`, `docs/wiki/what-works-now.md`, and
  `Mimir-Implementation-Context.md` to register the new crate and reflect
  Phase 3 as in progress.

## [0.61.3] — 2026-07-01

### Docs — Phase 3 (Connectors) implementation plan

- Added `VISION/09-Roadmap/Phase-3-Plan.md`: the Phase 3 design source of
  truth. Captures all eight locked architectural decisions (crate structure,
  extraction reuse/DRY, sync-state + DB-access boundary, orchestration, LLM
  call queuing, auth/secret storage, multi-backend architecture, tool-vs-
  connector disambiguation), the version-checked dependency ledger
  (`oauth2 5.0.0`, `async-imap 0.11.2`, `mail-parser 0.11.4`,
  `icalendar 0.17.12`, `kamadak-exif 0.6.1`, `keyring 4.1.2`), the 30-issue
  breakdown across five epics, and the dependency graph. Companion GitHub
  issues #178–#207 were created and tagged `phase-3`.

## [0.61.2] — 2026-07-01

### Fix — Avoid leaking partially-started workers (PR #177 review)

- (core): `LlmWorkerPool::new` now builds every worker's `LlmClient` up front
  into a `Vec` and spawns worker tasks only after all clients succeed. A
  later-iteration `LlmClient::new_direct` failure can no longer leave earlier
  spawned workers detached with no `LlmWorkerPool` handle to signal shutdown.
  Added `test_pool_spawns_exactly_configured_workers` regression test and a
  "Constructor Safety" section to `docs/llm-worker-pool.md`.


## [0.61.1] — 2026-07-01

### Fix — Address PR #177 review feedback

- (client): `normalize_base_url` now rejects non-hierarchical base URLs (e.g.
  `mailto:`) via `cannot_be_a_base()`, preventing late failures in `url()` /
  `session_messages()`. Added `try_new_rejects_non_base_url` test.
- (core): Handcrafted HTTP response in the pool in-flight counter test now uses
  CRLF line endings per HTTP/1.x framing, matching the suggestion from the review.


## [0.61.0] — 2026-06-30

### Optimization & robustness sweep (issues #161–#168)

- **#161** (core): Completed truncated doc comments on `JobError::is_not_registered`
  and `JobError::is_already_running` (`mimir-core/src/job_queue.rs`).
- **#162** (core): `DailySchedule::parse` now enforces a strict five-character
  `HH:MM` format with zero-padded fields; non-zero-padded inputs like `"2:30"`
  are rejected with `JobError::InvalidSchedule` for config-file determinism.
- **#164** (client): SSE stream parser already caps the buffer at 1 MiB
  (`MAX_SSE_EVENT_SIZE`) and scans delimiters linearly via `memchr` with a
  resume-from-cursor optimization — verified and documented.
- **#165** (client): Added fallible `MimirClient::try_new(base_url,
  connect_timeout, timeout) -> Result<Self, ClientError>`; `new` keeps the
  panicking default for back-compat. Build failures map to
  `ClientError::Connection`.
- **#166** (core): `LlmClient::new` and `new_direct` are now fallible
  (`Result<Self, LlmError>`); added `LlmError::ClientBuild`. Daemon startup
  (`start_server`, `AppState::from_config`) propagates the error instead of
  panicking; pool workers log and exit on a build failure.
- **#167** (client): DRY'd the repeated `check_response` + `resp.json::<T>()`
  pattern behind `send_response`/`send_json`/`get_json`/`post_json`/`check_status`;
  `stop` keeps bespoke 503 handling.
- **#168** (cli): `parse_datetime` interprets offsetless datetimes and
  date-only inputs in the local timezone (sharing the now-public
  `DailySchedule::naive_to_utc_local`); explicit RFC3339 offsets are preserved
  as UTC.

**Breaking (internal API):** `LlmClient::new` / `LlmClient::new_direct` now
return `Result`. Internal callers are updated; the OpenAI-compatible HTTP
endpoint is unaffected.

## [0.60.7] — 2026-06-29

### Fix — Handle already-fired shutdown trigger in `watch_shutdown`

Addressed PR #176 review feedback (CodeRabbit): `watch_shutdown` could miss a
SIGTERM/Ctrl-C fired in the gap between `spawn_os_signal_shutdown` and
`shutdown_tx.subscribe()`. `watch::Receiver::changed()` only wakes on *future*
updates, so a freshly subscribed receiver whose trigger already fired before
subscription would wait indefinitely (until sender drop, which never happens
during serving).

**Fix:** Check the current watch value via `borrow_and_update()` before
awaiting `changed()`. An already-fired trigger returns immediately; later
triggers are still caught by `changed()`.

- `mimir-server/src/lib.rs` — `watch_shutdown` now checks the current value
  first; added regression test
  `test_watch_shutdown_handles_already_fired_trigger`.
- `docs/shutdown.md` — documented the subscription-race guard.

## [0.60.6] — 2026-06-29

### Refactor — Single OS-signal listener for graceful shutdown

Addressed PR #176 review feedback (CodeRabbit): `serve_with_bounded_drain`
previously built two independent `shutdown_signal` futures — one for axum's
`with_graceful_shutdown` and one for the phase-1 serving loop — each
registering its own `ctrl_c()`/`SIGTERM` listener. The phase-1 waiter could
observe a signal before axum's graceful-shutdown future had registered
interest, leaving axum accepting connections until the drain bound kicked in.

**Fix:** Capture Ctrl-C / SIGTERM **once** in a dedicated
`spawn_os_signal_shutdown` task that fans the notification into the shared
`shutdown_tx` watch channel (the same channel `/stop` writes to). Both axum's
graceful-shutdown future and the phase-1 loop now observe that channel via
`watch_shutdown`, so they fire in lockstep with no duplicate OS-signal
listeners.

- `mimir-server/src/lib.rs` — replaced `shutdown_signal` with
  `spawn_os_signal_shutdown` and `watch_shutdown`; updated
  `serve_with_bounded_drain` to use the shared trigger.
- `docs/shutdown.md` — documented the single-listener trigger architecture.


## [0.60.5] — 2026-06-29

### Fix — Daemon no longer self-terminates 30 s after start

The graceful-shutdown drain bound was incorrectly applied to the **entire**
server lifetime: `tokio::time::timeout(Duration::from_secs(30), server_fut)`
wrapped the whole serving future, so the daemon unconditionally exited 30 s
after it began listening — whether or not a shutdown was ever requested. The
first `mimir chat`/`mimir ask` after start worked (inside the 30 s window);
any command issued later failed with `Mimir is not running.` because the
daemon had already exited with status 0 (so `Restart=on-failure` did not
relaunch it).

**Root cause:** `mimir-server/src/lib.rs` bounded the server future instead of
only the post-signal drain phase.

**Fix:** Extracted `serve_with_bounded_drain`, which splits shutdown into two
phases — an **unbounded serve** phase (poll the server concurrently with the
shutdown trigger) and a **drain bounded to `GRACEFUL_DRAIN_TIMEOUT` (30 s)**
phase (applied only after a trigger fires via Ctrl-C, `SIGTERM`, or `/stop`).
A wedged SSE stream can no longer keep the process alive past systemd's
`TimeoutStopSec`, and the daemon no longer dies on a fixed timer.

- `mimir-server/src/lib.rs` — `serve_with_bounded_drain`, `GRACEFUL_DRAIN_TIMEOUT`,
  regression test `test_serve_outlives_drain_timeout`.
- `docs/shutdown.md`, `docs/wiki/daemon-shutdown.md`, `docs/systemd-integration.md`
  — updated to describe the two-phase shutdown and the unbounded serving lifetime.


## [0.60.4] — 2026-06-29

### Docs — Knowledge Graph documentation audit & gap-fill (#64)

Completed the Knowledge Graph documentation set requested by issue #64 by auditing the existing equivalent docs against the issue's required content and filling stale/missing sections, plus adding the two genuinely missing wiki pages. Existing filenames were kept (DRY; avoids breaking cross-references).

**Technical docs (`docs/`):**
- `knowledge-graph-schema.md` — removed the dropped `entity_dates`/`entity_date_types` tables; documented the events & reminders overlay (`event_types`, `event_statuses`, `auto_complete_policies`, `events`, `pending_event_meta`); corrected the `predicates`→`relationship_types`/`relationship_constraints` rename (migration `031`); fixed lookup-row counts (`relation_types` 3→4, `change_types` 7→9); added `optimization_runs`/`optimization_pass_runs`/`memory_priorities`; completed the migration ordering (023–041); replaced the stale "Entity Dates & Recurrence" and "Future Work" sections.
- `Confidence-Model.md` — added a "Why No Time-Based Decay" rationale and a "Confidence Change Events" table mapping each trigger to its `ChangedBy` actor.
- `inference-engine.md` — added a "How to Add a New Rule" section and replaced the stale "Nightly Optimization" stub list with a reference to the implemented 10-pass pipeline.
- `nightly-optimization.md` — added a per-operation "Transaction Model" section, the `JobPriority` levels table, and a "Trigger & Daemon-Down Handling" note.
- `fact-extraction-pipeline.md` — audited; already current (LLM-orchestrated `remember` tool, sensitivity gate, confirm/reject flow).

**Wiki docs (`docs/wiki/`):**
- `knowledge-graph.md` — reframed as the "second brain" distinct from condensed memory; replaced the dropped entity-dates bullet with events & reminders; added an inference key-concept; replaced the stale "Future Commands (Planned)" with real `mimir kb` examples and a "Relationship to the Wider System" section; corrected the semantic-dedup "future work" note.
- `cli-commands.md` — fixed the stale `mimir memory` section (now KG-backed, not a file) and documented `--refresh`.
- `memory.md` — added "What Appears in Memory vs. What Stays in the Knowledge Graph"; fixed the `mimir kg query`→`mimir kb query` typo.
- `forgetting.md` (new) — soft-delete to a 30-day trash bin, restore, cascade forget, and bulk safeguards (>100 `--yes`, sensitive `--confirm-sensitive`, full-reset `DELETE EVERYTHING` + backup).
- `obsidian-sync.md` (new) — planned export/import design and file format, documented as **not yet implemented and deferred to post-Phase-5**.

## [0.60.3] — 2026-06-29

### Fixed — corroboration docs & nightly recalculation efficiency (#79, PR #174)

- **Documented the pending-confirmation corroboration path.** The confidence-model and "what works now" docs described corroboration only against an existing `Active` fact; `insert_fact_in_tx` also corroborates matching `pending_confirmation` facts. Both docs now state `Active` **or** `pending_confirmation`, matching the implementation.
- **Corrected the wiki corroboration cap statement.** Verified the wiki already states the non-explicit corroboration cap as `0.95` (not `1.1`); wording aligned with the pending-confirmation path for accuracy.
- **Nightly `confidence_recalc` skips already-refreshed rows.** Because each root-aware recalculation cascades to inferred descendants and clears their stale flags in one transaction, later iterations in the stale-fact snapshot could reopen transactions and re-walk subtrees already cleared by an ancestor pass. The loop now re-checks `stale_confidence` cheaply before recalculating, avoiding quadratic work on large stale branches.

## [0.60.2] — 2026-06-27

### Fixed — confidence cascade & nightly recalculation correctness (#79, PR #174)

- **Corroboration at the cap clears `stale_confidence`.** A corroborated fact already at the non-explicit cap (`0.95`) had its confidence unchanged, so the previous delta check skipped the whole update and left the row flagged stale despite new provenance. The update that clears `stale_confidence` now runs whenever corroboration applies, while the `ConfidenceChange` audit entry and the descendant cascade remain gated on an actual confidence delta.
- **Cascade uses a recursion-stack guard, not a global visited set.** `cascade_inner_tx` removed a fact from the visited set when its subtree finished, so a descendant reachable through multiple parents (a diamond graph) is recalculated once per updated parent and ends up with the correct final confidence instead of being skipped after the first parent updates.
- **Nightly `confidence_recalc` updates the stale root fact.** The pass previously only cascaded from each stale fact to its children and never recalculated/cleared the selected row itself, so the same facts could stay stale indefinitely. It now uses a root-aware transactional path (`confidence::recalculate_stale_fact`) that recalculates the stale row (inferred) or just clears its flag (non-inferred), writes a `ConfidenceChange` audit entry only when confidence changes, and then cascades to inferred descendants in the same transaction.

## [0.60.1] — 2026-06-27

### Fixed — corroboration guard consistency (#79)

- The corroboration guard in `insert_fact_in_tx` now treats `System`-sourced new facts as explicit, matching the boost-eligibility check and the documented contract. A `System` fact is no longer able to corroborate (and boost) an overlapping non-explicit fact; explicit facts (`UserEdit`/`System`) only add their source and supersede, never corroborate.

## [0.60.0] — 2026-06-27

### Added — corroboration detection in fact insertion (#79)

- **Corroboration is now resolved inside `insert_fact_in_tx`** for every insert
  path (extraction pipeline, batch insert, direct `KnowledgeGraph::insert_fact`),
  within the same transaction as supersession. When a new **non-explicit** fact
  covers the same claim as an existing `Active` (or pending-confirmation) fact —
  same `subject_id + relationship_type_id + object`, temporally overlapping
  `valid_from`/`valid_until` — Mimir adds a source row to the existing fact
  instead of creating a duplicate, and boosts the existing fact's confidence by
  `+0.05` per independent corroborating source, capped at `0.95`.
- **Explicit and inferred facts are excluded from the boost.** Explicit
  (`UserEdit`/`System`) facts stay at `1.0` — corroboration only adds the source
  for provenance. Inferred fact confidence is structural (recalculated from
  parents) and is never boosted by a corroborating source.
- **Re-statements are a no-op.** A source with identical provenance
  (`(source_type, connector_id, raw_reference)`) already recorded against the
  fact is not an independent corroboration and is skipped, which also avoids the
  `sources` UNIQUE-index collision.
- **Non-overlapping temporal ranges never corroborate** — they form a timeline
  of separate facts, matching the existing temporal-facts model.
- **Comprehensive in-transaction confidence cascade.** `cascade_confidence_change`
  is now transaction-aware (`cascade_confidence_change_in_tx`) and runs
  unbounded (cycle-guarded by a `visited` set) so a corroborated confidence
  change propagates to every inferred child for accuracy, with no artificial
  depth cap. The legacy depth-budget parameter was removed (the only caller, the
  nightly optimiser, already ran unbounded).
- **Audit + stale flag.** Corroboration writes `SourceAdded` and
  `ConfidenceChange` audit entries (the latter recording the triggering
  `source_id`) and clears `stale_confidence` on the existing fact.
- **Removed the pre-insertion `find_existing_fact` stub** in `extract.rs` and the
  now-unused `ExtractionOutcome.corroborated` / `ProcessResult::Corroborated`
  plumbing; corroboration is owned by the insert layer.

### Notes

- The `deterministic_dedup_merges_identical_fact_triples` test now sets up its
  duplicate via a direct SQL insert, because live same-claim non-explicit facts
  corroborate at insert time and can no longer coexist. The nightly dedup pass
  remains a safety net for coexisting duplicates (legacy data, direct writes).

## [0.59.1] — 2026-06-25

### Fixed — third pass on PR #173 review feedback

- **`confirm_fact` no longer errors after the confirmation commit.** The
  overlay-rebuild read of `pending_event_meta` ran after `tx.commit()`, so a
  `?`-propagated failure would make confirmation look failed to the caller even
  though the fact was already Active and no longer pending. The read now logs and
  falls back to the legacy one-time overlay path instead of returning an error
  (#3).
- **Legacy-fallback test now exercises the future-dated branch.** The
  `confirm_legacy_pending_fact_falls_back_to_one_time_reminder` test uses a
  future-dated fixture and asserts the one-time `Reminder` overlay is created;
  a second test covers the no-`valid_from` (no-overlay) case (#4).

### Notes

- CodeRabbit findings #1 (NULL confidence) and #2 (`event_type_roundtrips`
  `#[tokio::test]`) are stale re-posts: `facts.confidence` is `NOT NULL` and the
  test attribute is present at line 117. No code change required.

## [0.59.0] — 2026-06-25

### Fixed — second pass on PR #173 review feedback

- **Sensitive facts preserve event metadata across confirmation.** The
  extracted recurrence / `event_type` / `auto_complete_policy` /
  `requires_user_action` are now persisted in a new `pending_event_meta` table at
  extraction time and used by `confirm_fact` to rebuild the overlay faithfully,
  instead of synthesising one-time `Reminder` defaults. A confirmed sensitive
  recurring reminder keeps recurring; a confirmed sensitive task/deadline keeps
  requiring user action and surfaces as overdue. Legacy pending facts that
  predate the table fall back to the one-time `Reminder` overlay. This removes
  the Phase A limitation noted in 0.58.0 (#6).
- **`get_active_recurring` filters past-due rows in SQL.** The advance-pass
  query now takes the scan `now` and adds `trigger_date < now`, so the
  twice-daily scan only loads and sorts rows that can actually advance instead
  of fetching every future recurring event (#7).

### Added

- **Migration 041** — `pending_event_meta` table (fact-keyed event-shape cache
  for pending sensitive facts, removed on confirm / cascade-deleted on reject).
- **Public API:** `queries::event::{PendingEventMeta, insert_pending_event_meta,
  get_pending_event_meta, delete_pending_event_meta}`.

### Notes

- CodeRabbit findings #1 (advance filter) and #3 (`event_type_roundtrips`
  `#[tokio::test]`) were already satisfied by the current code and required no
  change; finding #2 (NULL confidence) is invalid because `facts.confidence` is
  `NOT NULL` and the derive and Upcoming queries already share the
  `confidence >= 0.5` gate.

## [0.58.0] — 2026-06-25

### Fixed — PR #173 review feedback on the events & reminders subsystem

- **Idempotent overlay derivation.** The derive scan now inserts overlays with
  `INSERT ... ON CONFLICT(fact_id) DO NOTHING` and only counts actual inserts,
  so a concurrent extraction can no longer trip the `fact_id` unique constraint
  (#3).
- **Recurring user-action events are no longer auto-advanced.** The advance pass
  now filters to `Recurring`-policy events with `requires_user_action = false`;
  recurring deadlines/tasks stay past their trigger date and surface as overdue,
  matching the documented contract (#4).
- **Sensitive time-bound facts get an overlay on confirmation.** Sensitive facts
  return `Pending` before the event block; `confirm_fact` now derives a one-time
  `AutoCompleteOnDate` overlay for future-dated sensitive facts when they are
  confirmed. Recurrence / `requires_user_action` are not carried across the
  sensitivity gate in Phase A (documented limitation) (#5).
- **Scan / Upcoming confidence alignment.** The derive query now applies the
  same `confidence >= 0.5` gate as the Upcoming render, so overlays are only
  created for facts that will surface (no hidden overlays for low-confidence
  interaction facts) (#7). Note: `facts.confidence` is `NOT NULL`, so the
  original "NULL confidence" framing was revised.
- **Calendar-day relative suffix.** `format_upcoming_line` computes the
  `today` / `in N days` suffix from `date_naive()` differences, so an event
  early the next calendar day is no longer mislabelled `today` (#8).
- **Docs.** `RecurringYearly` references in `docs/events-reminders.md` updated
  to `Recurring` to match the renamed policy (#1).

### Added

- **Env overrides for events.** `MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES`
  (comma-separated `HH:MM`) and `MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS` now flow
  through `apply_env_overrides_with`, matching the rest of the config (#2).
- **Public API:** `KnowledgeGraph::insert_event_if_absent` and
  `queries::event::insert_event_if_absent` for idempotent overlay creation.

### Notes

- `event_type` from extraction is intentionally limited to `Task`/`Reminder` in
  Phase A; the remaining `EventType` variants are seeded for later phases (#6).
## [0.57.0] — 2026-06-25

### Feature — events & reminders subsystem (issue #74)

A smart events/reminders layer for the knowledge graph. Events are modelled as
a lifecycle + recurrence overlay on facts: a fact with a future `valid_from` is
a one-time event; a fact tagged with recurrence (e.g. a birthday) is a
recurring event; a fact flagged `requires_user_action` is a task/deadline.
Upcoming events surface automatically in the "Upcoming" memory section.

- **New `events` table** (migration 039) with `event_types`, `event_statuses`,
  and `auto_complete_policies` lookup tables, keyed on `facts(id)`.
- **`entity_dates` deprecated and removed.** Its recurrence logic
  (`next_occurrence`) moved to `models::recurrence`; the unused
  `entity_dates` / `entity_date_types` tables are dropped (migration 040, no
  data migration required).
- **Deterministic scan job** `events.upcoming_scan` (default 06:00 & 18:00)
  derives overlays for future-dated facts, auto-completes one-time events past
  their trigger date, and advances recurring events to their next occurrence.
  `RequiresUserAction` events stay active and surface as overdue.
- **Extraction bridge.** The `remember` tool schema gains optional `recurrence`
  and `requires_user_action` fields; the extraction pipeline creates event
  overlays for qualifying facts (no natural-language date parsing in Rust —
  the LLM supplies the ISO-8601 `valid_from`).
- **`render_upcoming_section` refactored** to an event-based query, replacing
  the `entity_dates` and category 900–999 branches.
- **Config:** new `[knowledge.events]` section (`schedule_times`,
  `horizon_days`).

## [0.56.2] — 2026-06-24

### Bugfix — daemon service reliability

Fixes the three interlocking issues that made the installed `mimir.service`
fail to start cleanly and intermittently stop itself.

- **CLI no longer targets the wrong port.** Client commands resolved their
  base URL from a hardcoded `http://127.0.0.1:8080` (or `MIMIR_BASE_URL`),
  ignoring the configured `server.bind_addr`. A daemon bound to, e.g.
  `0.0.0.0:8008` was therefore reported as "not running", the daemon guard
  prompted to start an already-running service, and the auto-spawned duplicate
  failed to bind (address in use). The CLI now resolves `MIMIR_BASE_URL` →
  `server.bind_addr` (wildcard hosts normalised to loopback) → compiled default
  (`mimir/src/constants.rs`, `mimir-core::config::resolve_base_url`).
- **Cheap `/health` liveness endpoint.** The daemon guard and `mimir stop`
  probed `/status`, which performs a live LLM round-trip
  (`fetch_model_context_window`) plus knowledge-graph reads on every call. A
  slow/unreachable provider made the 500 ms probe time out on a healthy
  daemon. Added `GET /health` (trivial 200, no LLM/DB work) and pointed the
  guard + reachability check at it (`mimir/src/daemon_guard.rs`,
  `mimir-server/src/lib.rs`).
- **SIGTERM shutdown no longer deadlocks.** Only `POST /stop` broadcast the
  `shutdown_tx` watch channel; the SIGTERM/Ctrl-C path relied on `AppState`
  being dropped during runtime teardown to release the config file-watcher's
  `spawn_blocking` thread — a race that, when lost, deadlocked tokio's
  `BlockingPool::shutdown` until systemd aborted the unit with `SIGABRT` after
  `TimeoutStopSec` (the "it stops itself" symptom). The server now broadcasts
  `shutdown_tx` deterministically while the runtime is still alive, and wraps
  the server future in the documented 30 s `tokio::time::timeout`
  (`mimir-server/src/lib.rs`).
- **Config file-watcher no longer floods the journal.** Reading the config
  file generated `Access`/close events that, with only a filename filter, fed a
  self-reload loop (~1 reload/second even with no real change). The watcher now
  ignores `Access` events and dedupes by `(mtime, size)` so each genuine
  content change reloads at most once (`mimir-server/src/lib.rs`).

### Tests

- `mimir-core::config::base_url_tests` — base-URL resolution and config
  `bind_addr` reading (12 cases).
- `mimir-server::tests::test_health_returns_ok_without_llm` — `/health` does
  not touch the LLM backend.
- `mimir/tests/e2e.rs::e2e_sigterm_exits_promptly` — the real binary exits
  promptly on SIGTERM under an isolated environment.

## [0.56.1] — 2026-06-23

### Bugfix

- **Tool-call-start JSON printed to console.** The server emits a
  `tool_call_start` SSE event (containing `name` and `display_name`) before a
  tool executes, but the client SSE parser had no arm for that event type, so
  the raw JSON payload fell through to the default text path and was printed
  verbatim alongside the formatted result line. Added a `ToolCallStartInfo`
  type and `StreamItem::ToolCallStart` variant, a matching parser arm, and CLI
  handling so the event renders as a dim "🔧 DisplayName…" indicator instead
  of leaking JSON.

## [0.56.0] — 2026-06-23

### Bug & Performance Sweep

Address all open `bug` and `performance` labelled issues. Each was verified
before fixing; performance changes include before/after measurements.

#### Bugs

- **#45 — `get_current_time` returns UTC instead of user's time zone.** The
  tool now returns a structured payload (`local`, `utc`, `offset`) derived
  from the host's local timezone via `chrono::Local`, so the agent can derive
  UTC from the offset. The formatting helper is generic over the timezone for
  deterministic unit testing.
- **#80 — Some config settings don't do anything (temperature).** The LLM
  client captured `temperature` at startup, so hot-reloaded changes had no
  effect. Added `LlmBackend::with_temperature_override`; the chat route now
  applies the live config snapshot temperature per request.
- **#81 — Certain CLI commands don't work.** `mimir chat` accepted no flags
  and always sent `model`/`personality`/`incognito` as `None`, ignoring
  `--verbose`. Added `--model`, `--verbose`, `--incognito`, `--personality`
  flags plus REPL slash-commands (`/model`, `/personality`, `/incognito`,
  `/verbose`) that toggle at runtime; verbose now reports token usage.
- **#155 — Incognito mode can still write facts via `remember`.** Added a
  `Tool::is_write_tool` marker (default `false`); `RememberTool` opts in. The
  chat routes now suppress write-capable tools from the exported tool set and
  refuse to execute them during incognito turns, so no facts are persisted.

#### Performance

- **#160 — `api-types` leaks `null` fields in KG wire types.** Added
  `#[serde(skip_serializing_if = "Option::is_none")]` to the sparse `Option`
  fields of `AuditRow`, `CategoryResponse`, `CategoryDetailResponse`,
  `TrashRow`, `OptimizationStatusResponse`, `OptimizationRunNowResponse`, and
  `OptimizationRunSummary`. Sparse payload sizes shrink 42–75% per row
  (e.g. a sparse `AuditRow` is 64 B vs 123 B; a sparse `CategoryResponse` is
  19 B vs 76 B).
- **#163 — `escape_fts5` keeps leading/trailing whitespace in the phrase.**
  The quoted phrase now uses the trimmed value so padded queries no longer
  alter FTS5 phrase-matching semantics. `fts5_escape_mixed_inputs` benchmark:
  1.41 µs → 1.26 µs (~7% incidental).
- **#164 — SSE stream parser has unbounded buffer growth and O(n²) scan.**
  The client SSE parser now caps a single event at 1 MiB (emitting a
  `ClientError` on overflow) and resumes the delimiter scan from the last
  inspected offset (using `memchr::memmem`), making the cost linear in the
  event size. Benchmark on the partial-event accumulation path:
  - 1024 chunks: 31.5 ms → 542 µs (~58×)
  - 4096 chunks: 494.9 ms → 1.21 ms (~408×)

### Notes

- `memchr` 2.8 added to `mimir-client`.
- Five pre-existing `mimir-server` KB pending/confirm/reject tests fail on
  `main` independently of this change (index-out-of-bounds in
  `insert_pending_fact`); left untouched per scope.

## [0.55.1] — 2026-06-23

### Sensitivity Content Check: Word-Boundary Matching (#142)

Fix a false-positive vector flagged in PR review. The keyword-based content
fallback (`is_sensitive_by_content`) previously matched keywords as raw
substrings, so benign words containing a sensitive keyword (e.g. "hospitality"
contains "hospital", "indebted" contains "debt", "visage" contains "visa")
could be confirmed sensitive whenever the LLM also set `is_sensitive=true`.

- **`mimir-knowledge/src/sensitivity.rs`:** `is_sensitive_by_content` now
  matches each keyword as a whole word using ASCII alphanumeric boundaries via
  the new private `contains_keyword_word` helper, eliminating embedded-word
  false positives while still catching genuine single-word uses like "diabetes"
  or "allergic".
- **Tests:** Added word-boundary regression tests for "hospitality", "indebted",
  and "visage", plus a trailing-punctuation case and a genuine "hospital" word
  case.

## [0.55.0] — 2026-06-22

### Rework Sensitivity Detection (#142)

Move sensitivity detection from LLM-only to deterministic Rust validation,
eliminating false positives where benign preferences were routed into the
pending-confirmation dead end.

- **New module `mimir-knowledge/src/sensitivity.rs`:** Pure, synchronous
  sensitivity gate with two signals:
  - `is_sensitive_by_category(category_ids)` — checks the fact's catalogue
    category IDs against the `SENSITIVE_CATEGORIES` constant (health, allergies,
    financial, romantic, cultural/religious, values/philosophy).
  - `is_sensitive_by_content(object)` — keyword-based fallback for
    miscategorised facts (e.g. "allergic", "diabetes", "salary", "debt",
    "divorce", "citizenship").
  - `is_sensitive(llm_flag, category_ids, object)` — combined AND gate: a fact
    is sensitive only if the LLM flags it **and** Rust agrees. Rust can narrow
    but never widen.
- **Extraction prompt softened:** "Flag ... Mimir will validate your assessment
  in Rust."
- **Sensitivity check wired into `process_extracted_fact`** — the single funnel
  point covering `extract_facts`, `extract_facts_with_context`, and
  `process_remember_output`.
- **35 unit tests + 7 integration tests** covering all issue acceptance
  criteria.

## [0.54.5] — 2026-06-22

### Review Fixes (PR #169)

Address CodeRabbit review feedback on the tests-and-benchmarks change set.

- **`mimir-api-types`:** `roundtrip_tests!` sparse-field check now parses the
  serialised JSON into a `serde_json::Map` and asserts key absence via
  `contains_key`, instead of the previous substring-based `json.contains`
  that could match field names inside values.
- **`mimir-client`:** wiremock-backed KB endpoint tests
  (`kb_query`, `kb_browse`, `kb_profile`, `kb_audit`, `kb_trash`) now assert
  the expected query-string parameters via `query_param` matchers, catching
  regressions in query encoding rather than just the route path.
- **`mimir-core`:** `bench_daily_schedule_next_after` uses a fixed
  `DateTime<Utc>` reference instead of `Utc::now()`, so the benchmark baseline
  is deterministic and reproducible across runs. Added
  `daily_schedule_parse_accepts_non_zero_padded_input` to document chrono's
  padding-agnostic `%H:%M` parsing.
- **`mimir` (binary):** Corrected the misleading comment in
  `truncate_zero_max_yields_just_ellipsis_or_empty` to state the deterministic
  "just ellipsis" outcome.

## [0.54.4] — 2026-06-21

### Testing & Benchmarks

Workspace-wide expansion of inline unit tests and pure-helper benchmarks on
the `tests-and-benchmarks` branch.

- **`mimir-api-types`:** 12 → 46 unit tests. New `roundtrip_tests!` macro
  asserts populated + sparse (all-`None`) serde roundtrips and
  `skip_serializing_if` omission for every KG wire type.
- **`mimir-client`:** ~24 → 64 tests. Adds pure unit tests for the SSE parser
  primitives (`find_double_newline`, `parse_sse_event`) and wiremock-backed
  tests for all previously-uncovered `MimirClient` methods.
- **`mimir-core`:** 179 → 211 lib tests. New inline tests for `job_queue`,
  `tools::{output,permission,error}` pure helpers.
- **`mimir-knowledge`:** ~74 → 110 lib tests. New inline tests for
  `models::enums`, `retrieval::types`, `inference::rules::transitivity`,
  `models::{entity_date,memory}` helpers.
- **`mimir-server`:** 50 → 65 lib tests. New `error.rs` tests covering every
  `ApiError` response helper, including verification that internal error
  details are masked from clients.
- **`mimir` (binary):** 15 → 29 bin tests. New `kb.rs` tests for
  `parse_datetime`, `confidence_color`, and `truncate`.
- **Benchmarks:** three new pure-helper suites — `mimir-api-types/wire_types`,
  `mimir-core/pure_helpers`, `mimir-knowledge/pure_helpers` — covering
  non-hotpath pathways (FTS5 escaping, confidence scoring, serde roundtrips,
  schedule arithmetic, tool-output rendering).

### Documentation

- New `docs/unit-tests.md` and `docs/wiki/Testing-and-Benchmarks.md`;
  `docs/benchmarks.md` updated with the new pure-helper suites.

### Issues

Triaged nine prescriptive follow-ups as GitHub issues #160–#168
(api-types `skip_serializing_if` consistency, doc-comment completion,
`DailySchedule::parse` strictness, `escape_fts5` whitespace, SSE buffer DoS,
client/LLM construction robustness, client DRY, `parse_datetime` timezone).

## [0.54.3] — 2026-06-21

### Security

- **`mimir-server`:** the sensitive-fact confirmation lifecycle routes
  (`GET /kb/pending`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject`)
  are now wrapped in the `require_loopback` middleware, matching the guard
  already applied to `/kb/optimization/run-now`, `/memory/refresh`, and `/stop`.
  Only loopback peers can list or mutate pending sensitive facts. No CSRF /
  `Origin` validation is applied because there is no browser frontend for these
  routes (the CLI / `mimir-client` is the only client); that hardening belongs
  to a workspace-wide pass over all mutation routes.

## [0.54.2] — 2026-06-21

### Fixed

- **`mimir-knowledge`:** `extract::reject_fact` now clears `fact_dependencies`
  rows before the hard-delete. The `fact_dependencies` FK is `ON DELETE
  RESTRICT` (migration 017), so rejecting a pending sensitive fact that
  participates in a dependency edge previously hit a foreign-key violation.
  Mirrors the dependency cleanup already performed by
  `KnowledgeGraph::delete_stale_pending` and `forget_fact_tx`.
- **`mimir-knowledge`:** `KnowledgeGraph::delete_stale_pending` now re-checks
  the stale predicate inside each per-fact transaction and only counts
  committed deletes. A fact confirmed or rejected between the id scan and the
  delete is skipped rather than incorrectly hard-deleted and given a spurious
  `Rejected` audit entry.
- **`mimir-knowledge`:** the optimization runner's `pending_confirmation_cleanup`
  pass now uses the configured `knowledge.pending_cleanup.retention_days`
  (via a new `OptimizationConfig.pending_cleanup_retention_days` field) instead
  of a hardcoded 7 days, so the pass and the scheduled `knowledge.pending_cleanup`
  job share one configured expiry window.
- **docs:** `docs/wiki/facts.md` confirm/reject examples now use the positional
  `<fact-id>` syntax, matching `cli-commands.md` and `README.md`.

## [0.54.1] — 2026-06-21

### Fixed

- **`mimir-knowledge`:** removed the orphaned, never-called
  `queries::fact::delete_stale_pending` helper. It duplicated
  `KnowledgeGraph::delete_stale_pending` with divergent, FK-violating semantics
  (skipped `fact_dependencies` cleanup and the `Rejected` audit entry).
  `KnowledgeGraph::delete_stale_pending` is now the single source of truth for
  stale pending-fact auto-expiry.

## [0.54.0] — 2026-06-21

### Added

- **Pending sensitive-fact confirmation lifecycle (`mimir-server`, `mimir-client`,
  `mimir`, `mimir-api-types`, `mimir-knowledge`):** the existing internal
  `confirm_fact`/`reject_fact` APIs are now exposed end-to-end. Sensitive facts
  (allergies, health, etc.) stored with `pending_confirmation = TRUE` no longer
  sit in limbo.
  - HTTP routes: `GET /kb/pending`, `POST /kb/facts/{id}/confirm`,
    `POST /kb/facts/{id}/reject` (returns `204 No Content`; optional `reason`
    body field written to the audit log).
  - CLI commands: `mimir kb pending`, `mimir kb confirm --fact-id N`,
    `mimir kb reject --fact-id N [--reason "..."]`.
  - API types: `PendingListResponse`, `ConfirmFactResponse`, `RejectFactRequest`,
    and a public `PendingFactRow`.
  - New `KnowledgeGraph::list_pending_facts()` and `delete_stale_pending()` query
    methods.

- **Daily pending-fact auto-cleanup job (`mimir-server`):** a new
  `knowledge.pending_cleanup` background job hard-deletes facts still awaiting
  confirmation past a configurable retention window. Configurable under
  `[knowledge.pending_cleanup]` with `retention_days` (default `7`) and
  `schedule_time` (default `"03:30"`). Implements the 7-day auto-deletion rule
  described in `VISION/02-Knowledge-Graph/Learning-Modes.md`.

### Changed

- **`reject_fact` now accepts an optional reason (`mimir-knowledge`):** the
  free function and `KnowledgeGraph` method take `reason: Option<&str>`,
  threaded through to the audit log. Internal API change (acceptable per the
  breaking-changes policy).


## [0.53.1] — 2026-06-19

### Fixed

- **Librarian transcript now escapes newlines in message content
  (`mimir-knowledge/src/extract.rs`):** `\r` and `\n` in `msg.content` are
  replaced with literal `\r`/`\n` sequences before the `[Role]:` label is
  applied, preventing a user message containing text like
  `[Assistant]: …` from forging a labelled line and bypassing the source
  discipline boundary. Adds a regression test
  (`prompt_escapes_multiline_content_so_roles_cannot_be_forged`).

### Changed

- **Librarian wiki documentation aligned with implemented novelty check
  (`docs/wiki/librarian-agent.md`):** the duplicate-handling paragraph now
  states that facts restating the core-facts block are skipped (not that
  confidence is "strengthened"), matching the novelty check instruction.

### Tests

- **Assert message-turn shape before indexing (`mimir-knowledge/tests/librarian_agent.rs`):**
  the `calls[0].len() == 2` assertion now precedes the indexing into
  `calls[0][0]`/`calls[0][1]` so a shape change fails clearly instead of
  panicking out-of-bounds.

## [0.53.0] — 2026-06-19

### Changed

- **Librarian extraction prompt redesigned (Issue #139):** the Librarian's
  `build_extraction_prompt` now composes a KG-focused base (rules, category
  taxonomy, predicate standards, list splitting, deduplication, output contract
  — extracted into a shared `build_base_prompt`) with the *same* core-facts
  block the core agent injects (`Personality::CORE_FACTS_HEADER` + condensed
  memory, emitted only when non-empty) and the recent conversation rendered as
  labelled `[User]` / `[Assistant]` messages under `## Recent conversation`.
  The user's identity is read from the core-facts block by the LLM, exactly as
  the core agent resolves identity — `UserIdentity` is no longer threaded
  through the contextual extraction path. A "Source discipline" instruction
  tells the Librarian to extract facts ONLY from `[User]` messages and never
  from `[Assistant]` messages (its own prior output), and a "Novelty check"
  instruction tells it to extract only facts not already present in the
  core-facts block. The instruction tells the LLM to skip emitting facts that
  merely restate what is already known (exact duplicates are discarded by Rust
  regardless of classification), and to use the Correction classification for
  corrections — avoiding contradictory "strengthen confidence" guidance.
  The transcript now lives in the system prompt once; the user turn handed to
  the LLM is a short action instruction, removing the previous duplication.

### Added

- `mimir_core::conversation::{ConversationMessage, MessageRole}` — a labelled
  transcript message type. `extract_facts_with_context` and
  `KnowledgeGraph::extract_facts_with_context` now take `&[ConversationMessage]`
  instead of a `ConversationTurn` + `UserIdentity`, so the amount of
  conversation context handed to the Librarian can be increased in future
  without changing the prompt-builder signature. `LibrarianAgent::run`
  converts the turn into `[User, Assistant]` messages today.
- `Personality::CORE_FACTS_HEADER` is now `pub` so the Librarian reuses the
  core agent's core-facts label (DRY).

### Removed

- `build_contextual_extraction_prompt` (folded into `build_extraction_prompt`).
- The "Recent related facts about the user" DB snapshot
  (`get_facts_by_subject`) from the Librarian prompt — novelty checking now
  relies on the core-facts block.
- `identity: UserIdentity` field from `LibrarianContext` and the
  `identity` parameter from `extract_facts_with_context`.

### Notes

- This deviates from the original #139 spec (which was outdated): identity is
  not rendered as a separate prompt line, there is no dedicated recent-facts
  snapshot section, and the exact `## What I already know about you` /
  `## Recently learned` / `## Conversation to analyze` headers from the issue
  are not used. Pronoun-resolution prompting is deferred to the follow-up
  "Phase 2: Pronoun resolution in fact extraction" issue.

## [0.52.0] — 2026-06-18

### Changed

- **System prompt hardened for the agentic architecture (Issue #138):** the
  system prompt composed by `Personality::system_prompt` now appends shared
  operating directives to every preset (built-in and custom). The directives
  tell the LLM not to invent facts about the user (say so if the answer is not
  known), to dispatch a retrieval agent via the `retrieve_context` tool when
  the core facts are insufficient (refining and re-dispatching until answered
  or confirmed absent), and to call the `remember` tool for explicit
  assertions, corrections, and meaningful casual mentions — never for chitchat.
  The injected memory section is relabelled `Core facts about the user` (third
  person, framed as a condensed subset, not exhaustive). The legacy `Key facts
  I know about you:` block and its note mentioning `kg_query`/`kg_search`/
  `kg_related` are removed; those tools are the retrieval agent's internal
  tools and are no longer surfaced to the core LLM. The four `built_in_*`
  presets keep their tone text unchanged — directives are composed once in
  `system_prompt` for DRY.

### Notes

- **#138 acceptance criteria revised by design:** the "you will receive a
  synthesized context block" criterion is dropped — Mimir uses LLM-condensed
  core facts, not a Rust distillation layer (#129 will not ship as written).
  The "remove the `remember` instruction" criterion is reversed to *encourage*
  `remember`, matching the #137 inline-LLM-orchestrated learning design and the
  `test_chat_extracts_facts_after_response` contract. An automatic Librarian
  fallback (#156) was filed to queue background extraction when `remember` is
  not called for a configurable number of turns.

## [0.51.0] — 2026-06-18

### Changed

- **Learning is now LLM-orchestrated (Issue #137):** the unconditional
  background Librarian that re-extracted facts after every non-incognito chat
  turn has been retired. Fact learning now happens when the conversational LLM
  calls the `remember` tool inline while composing its reply, and pre-response
  retrieval stays LLM-driven via the `retrieve_context` tool. The LLM decides
  *whether* to learn/retrieve; Rust still owns the policy (confidence
  assignment, overwrite rules, sensitive-fact confirmation) via
  `process_remember_output`, so the model cannot self-assign confidence or
  override policy. This reframes issue #137 away from a Rust rule-based intent
  classifier — NLU is the LLM's job, and orchestration emerges from structured
  tool selection.
- **`remember` tool description** now summarises the classification semantics
  (Explicit overwrites, Casual coexists, Correction supersedes) and nudges
  canonical relationship types, preserving extraction quality without a second
  LLM call.

### Removed

- `submit_librarian_goal` and its two call sites in the chat route. The
  `LibrarianAgent`, `LibrarianGoal`/`LibrarianContext`, and
  `KnowledgeGraph::extract_facts_with_context` remain as a library API for
  future on-demand/bulk extraction; they are simply no longer auto-invoked.

### Notes

- End-user-visible behaviour: Mimir no longer silently learns from chitchat. It
  learns when it judges a turn contains worth-remembering information.
- Sensitive facts still require confirmation; the overwrite/coexistence matrix
  in `VISION/02-Knowledge-Graph/Learning-Modes.md` is unchanged and enforced in
  Rust.

## [0.50.0] — 2026-06-18

### Changed

- **Predicate resolution is fully data-driven (Issue #136):** the deprecated
  hardcoded `normalize_predicate` synonym map and the duplicate
  `normalize_relationship_type` snake_case helper have been removed from
  `mimir-knowledge/src/extract.rs`. The extraction pipeline now resolves every
  fact's `relationship_type` through `KnowledgeGraph::ensure_relationship_type`,
  which consults the `relationship_type_aliases` table (seeded by migrations
  `036`/`037`) and auto-registers unknown predicates as new canonical types.
- **DRY batch processing:** `process_extracted_facts` and
  `process_remember_output` now share a single `process_fact_batch` helper, and
  predicate normalization reuses `normalize_alias` from `mimir-knowledge/src/lib.rs`
  instead of a local copy. Predicate-resolution errors are tolerated per-fact, so
  one malformed predicate no longer aborts the whole extraction batch.

### Notes

- End-user behaviour is unchanged: `attended`→`studied_at`, `hobbies`→`hobby`,
  `works_for`→`works_at`, etc. all resolve via seeded aliases.
- Side effect of routing through `ensure_relationship_type`: an unknown predicate
  on a fact that is later rejected (e.g. invalid `subject_type`) still registers its
  canonical type. This is intentional and idempotent.

## [0.49.1] — 2026-06-18

### Fixed

- **Address PR #152 review feedback (Issue #135 ontology seed):**
  - Migration `037` now uses `ON CONFLICT` UPSERTs (not `INSERT OR IGNORE`) for the
    canonical predicates and their self-aliases, enforcing the canonical `(id, name)`
    contract on upgrade instead of silently preserving stale mappings.
  - Migration `038` runs inside a transaction with foreign-key enforcement on
    (`PRAGMA foreign_keys = OFF` removed) and uses `CREATE TABLE/INDEX IF NOT EXISTS`
    for defensive idempotency.
  - `insert_category_alias` now uses an atomic `INSERT OR IGNORE` + post-insert
    resolution, eliminating the `SELECT`-then-`INSERT` race that could surface raw
    `UNIQUE`-constraint errors instead of the documented `Validation` error.
  - `category_aliases_test` re-queries the subtree after inserting the unrelated
    fact so the exclusion assertion is meaningful.
  - `relationship_ontology_test` self-alias check is now read-only (direct canonical
    id lookup) instead of mutating the DB via `ensure_relationship_type`.

## [0.49.0] — 2026-06-18

### Added

- **Core relationship ontology (category-first, Issue #135)**: the knowledge graph is
  now seeded with a category-first ontology. Predicate aliases own verb canonicalization
  (thin canonical verbs + English synonyms); the Dewey `categories` tree owns grouping,
  hierarchy, and multi-tag precision.
  - Migration `037` seeds the remaining core predicates (`studied`, `completed_degree`,
    `educational_status`, `job_title`, `likes`, `dislikes`) with explicit ids 26–31 and
    self-aliases, so the alias table remains the single source of truth for resolution.
  - Migration `038` adds the `category_aliases` table (`alias` → `category_id`,
    globally unique) and seeds domain words (`education`, `hobbies`, `residence`,
    `family`, `identity`, `employment`, `pets`, …) mapping to existing Dewey category
    nodes. Both migrations are idempotent (`INSERT OR IGNORE`).
  - New `queries::category` helpers: `resolve_category_alias`, `insert_category_alias`,
    `get_descendant_category_ids` (recursive CTE over `categories.parent_id`), and
    `get_facts_in_category_subtree` (facts tagged anywhere in a root + descendants).
    `KnowledgeGraph` exposes thin wrappers for each.
  - Unit tests verify predicate/alias counts, alias resolution, category-alias counts,
    subtree retrieval, and idempotency across re-init.

### Changed

- **Design shift documented**: grouping/hierarchy is intentionally served by categories,
  not abstract parent predicates. `relationship_type_hierarchy` is kept but no longer
  seeded with abstract parents; reworking `kg_query --include_subtree` to expand by
  category subtree (rather than the predicate DAG) is a tracked follow-up (#134, #136).
- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/what-works-now.md`, new
  `docs/wiki/categories-and-aliases.md`, and `README.md` to reflect the category-first
  layering.

## [0.48.1] — 2026-06-18

### Fixed

- **`kg_query` subtree offset contract**: when `include_subtree` is `true` and the
  requested predicate does not exist (empty result set), the response `offset` is now
  forced to `0` instead of echoing the caller-supplied `offset`. This closes a gap where
  the documented "subtree mode disables offset pagination" contract was only honoured on
  the populated-result path.

## [0.48.0] — 2026-06-17

### Added

- **Relationship-type DAG subtree query (Issue #134)**: facts can now be retrieved
  for a relationship type and all of its descendants in the
  `relationship_type_hierarchy` DAG via a SQLite recursive CTE. Querying a broad
  category (e.g. `education`) returns facts stored under more specific descendant
  types (`studied_at`, `graduated_from`, …) without the caller needing to know every
  predicate name.
  - `queries::fact::get_facts_by_relationship_subtree(pool, subject_id, root_type_id,
    min_confidence, limit)` and the matching `count_facts_by_relationship_subtree` walk
    the DAG in a single statement, seeding the CTE with the root type so its own facts
    are included. Filters and ordering match `get_facts_by_subject_filtered` (non-pending,
    status `NOT IN (5, 6)`, confidence floor, sorted by confidence descending).
  - `KnowledgeGraph::get_facts_by_relationship_subtree(entity_id, root_type_id, limit)`
    is a convenience wrapper with `min_confidence = 0.0`.
  - `kg_query` gains an `include_subtree` boolean parameter (default `false`). When set
    with a `predicate`, the predicate (alias-aware) becomes the subtree root; an unknown
    predicate returns an empty result set, and `include_subtree` without a `predicate`
    is rejected with `ToolError::InvalidArguments`.

### Changed

- Extracted a shared `enrich_with_sources` helper in `queries/fact.rs` so the exact-match
  and subtree fact queries share the source-batching logic (DRY).

### Tests

- Added `mimir-knowledge/tests/relationship_subtree_test.rs` covering subtree inclusion of
  root + descendants, diamond-path deduplication, status/pending/confidence/limit filters,
  temporal-bound preservation, multi-valued same-type facts, the `KnowledgeGraph` wrapper,
  and the `kg_query` `include_subtree` parameter (including alias resolution and the
  predicate-required contract).

### Documentation

- Updated `docs/kg-tools.md`, `docs/knowledge-graph-schema.md`, `docs/wiki/kg-tools.md`,
  and `docs/wiki/knowledge-graph.md` with the subtree query and `include_subtree` parameter.

## [0.47.0] — 2026-06-16

### Added

- **Relationship type alias resolution (Issue #133)**: `ensure_relationship_type` now resolves
  incoming names through the `relationship_type_aliases` table before creating a new canonical
  type. New canonical types automatically register their normalized name as a self-alias, making
  the alias table the single source of truth for relationship-type lookup.
- Migration `036_seed_relationship_type_aliases.sql` backfills self-aliases for every existing
  relationship type and seeds the legacy hardcoded synonyms from `extract.rs::normalize_predicate`
  (e.g., `attended` → `studied_at`) as data-driven aliases.

### Changed

- `mimir-knowledge/src/extract.rs::normalize_predicate` is now deprecated. Fact extraction
  normalizes predicates to snake_case and resolves aliases through the alias table before list
  expansion; the hardcoded synonym map remains only as a deprecated fallback.
- `get_relationship_type_id` now resolves aliases through `relationship_type_aliases`, matching
  `ensure_relationship_type` behavior.

### Tests

- Added `ensure_relationship_type_resolves_alias_to_canonical` and
  `ensure_relationship_type_creates_new_type_and_self_alias` to
  `mimir-knowledge/tests/relationship_type_dag_test.rs`.
- Updated existing tests and lookup-sync expectations to account for the seeded relationship
  type ontology.

### Documentation

- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/knowledge-graph.md`, and
  `docs/fact-extraction-pipeline.md` to describe alias-aware resolution and the deprecated
  hardcoded fallback.

## [0.46.1] — 2026-06-16

### Fixed

- **Relationship type alias/canonical collision checks (PR #149 review)**: centralised
  alias↔canonical collision validation in `mimir-knowledge` and applied it inside the
  same transaction for every relationship-type write path (`ensure_relationship_type`,
  `ensure_relationship_type_in_tx`, `insert_relationship_type`, and
  `insert_relationship_type_alias`). Previously these checks could be bypassed when
  creating relationship types directly or through the transactional fact-insert path.

### Tests

- Added `relationship_type_dag_test.rs` cases covering:
  - `insert_relationship_type` rejecting a canonical name that shadows an existing alias.
  - `insert_relationship_type` rejecting an alias that shadows an existing canonical name.
  - `insert_facts_batch` (transactional create path) rejecting a relationship type name
    that shadows an existing alias.

### Documentation

- Updated `docs/knowledge-graph-schema.md` with the collision invariants section.
- Updated `docs/wiki/knowledge-graph.md` with a relationship types overview.

## [0.46.0] — 2026-06-16

### Added

- **Relationship type DAG schema (Issue #132)**: added `relationship_type_hierarchy`
  and `relationship_type_aliases` tables to `mimir-knowledge`. Relationship types now
  support a directed acyclic graph (multiple parents allowed) and globally unique English
  aliases, enabling data-driven predicate discovery instead of hardcoded synonym tables.
- `RelationshipType` and `NewRelationshipType` models in `mimir-knowledge/src/models/relationship_type.rs`.
- New `KnowledgeGraph` API methods: `insert_relationship_type_hierarchy`,
  `insert_relationship_type_alias`, `resolve_relationship_type_alias`,
  `get_descendant_relationship_type_ids`, and `get_ancestor_relationship_type_ids`.
- Cycle detection for hierarchy inserts, returning `KnowledgeError::RelationshipTypeCycle`.
- Alias resolution integrated into fact extraction; the legacy hardcoded `normalize_predicate`
  map remains as a deprecated fallback until the core ontology is seeded.

### Tests

- New `mimir-knowledge/tests/relationship_type_dag_test.rs` covering migrations,
  DAG traversal, alias resolution, global alias uniqueness, self-loops, indirect
  cycles, and alias-based predicate normalization.
- Updated `mimir-knowledge/tests/migrations_test.rs` to assert the new tables exist.

### Documentation

- Updated `docs/knowledge-graph-schema.md` with the relationship type DAG design.
- Updated `docs/wiki/what-works-now.md` to list the new DAG + aliases feature.
- Updated `README.md` to mention the relationship ontology layer.

## [0.45.1] — 2026-06-15

### Fixed

- `ConversationTurn` equality and hashing now ignore the `timestamp` field, restoring
  `AgentRuntime` deduplication of identical `(agent kind, goal)` pairs.
- `AgentRuntime::submit` always removes the pending goal key, even when the agent task
  panics, preventing permanent leaks in the pending set.
- Chat routes skip Librarian fact extraction when the user message is empty, avoiding
  wasted LLM calls for empty chat turns.

## [0.45.0] — 2026-06-15

### Added

- **Librarian Agent (Issue #130)**: Replaced the fire-and-forget `spawn_fact_extraction` helper with a reusable background agent. The `Agent` trait and `AgentRuntime` live in `mimir-core`; `LibrarianAgent` lives in `mimir-knowledge`. After each non-incognito chat turn, the route submits a `LibrarianGoal` carrying the full `ConversationTurn`, and the agent extracts facts using the configured user identity, condensed memory, and recent related facts.
- New shared types: `mimir_core::conversation::ConversationTurn` and `mimir_core::identity::UserIdentity`.
- New extraction entrypoint: `mimir_knowledge::KnowledgeGraph::extract_facts_with_context` builds a rich contextual prompt for the `remember` tool.
- New integration tests in `mimir-knowledge/tests/librarian_agent.rs` verify fact extraction from a conversation turn and prompt content.

### Changed

- Chat routes (`/chat` and `/chat/stream`) now submit a Librarian goal instead of spawning `spawn_fact_extraction` directly.
- `test_state_with_config` in `mimir-server` now resolves or creates the configured user entity so background agents can run in server integration tests.

### Documentation

- Added `docs/librarian-agent.md` (technical design) and `docs/wiki/librarian-agent.md` (user-facing overview).
- Updated `docs/fact-extraction-pipeline.md` and `docs/wiki/what-works-now.md` to describe the Librarian Agent.
- Updated `README.md` and `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` to reflect the new agent framework and note future goal-directed research.

## [0.44.0] — 2026-06-14

### Changed
 
 - **Scheduler async mutex**: `BackgroundScheduler` now uses `tokio::sync::Mutex` for pending/running job state and `submit()` is `async`, eliminating clippy `await_holding_lock` warnings and preventing accidental blocking of the async runtime.
 - **Centralized path resolution**: All config/data path construction now routes through `mimir_core::paths`. New helpers added: `skills_dir()`, `history_path()`, `personalities_dir()`. `ToolsConfig::default_path()` and `SkillsPermissionsConfig::default_path()` no longer duplicate `dirs::config_dir()` logic.
 - **Skill permission config placement**: `SkillsPermissionsConfig` moved from the `mimir` binary crate into `mimir_core::skills::permissions_config`, consolidating skill-related persistence in the core library.
 - **DRY HTTP client handling**: `mimir_client` response status handling is centralized in `MimirClient::check_response`, removing duplicated error blocks across every API method.
 - **DRY tool registration**: `mimir-server` startup now registers native tools through a single `register_tool` helper instead of repeating `if let Err(e) = ...` warning blocks.
 - **DRY daemon guard checks**: The `mimir` CLI dispatch loop uses a single `ensure_daemon` helper instead of repeating the same `ensure_daemon_running` error-handling block for every daemon-requiring subcommand.
 - **Shared category API types**: `CategoryResponse` and `CategoryDetailResponse` moved from `mimir-server` into `mimir_api_types` so the server and HTTP client share the same wire types.
 - **Category CLI uses MimirClient**: `mimir kb category` subcommands now use `MimirClient` instead of raw `reqwest` calls, and category methods (`kb_categories`, `kb_category_show`, `kb_category_create`, `kb_category_delete`) were added to the client.
 - **DRY `mimir kb` client construction**: All `mimir kb` handlers now share a local `make_client(base_url)` helper instead of repeating `MimirClient::new(base_url)` at every call site.
 - **DRY kb CLI error handling**: All `mimir kb` handlers use a shared `exit_with_error` helper instead of repeating the same `eprintln!`/`exit(1)` block.
 - **DRY skill/tool command helpers**: `mimir tool/skill` enable/disable/permission handlers use shared `set_*_permission_or_exit` and `persist_*_or_exit` helpers; skill name validation and origin parsing are also shared.
 - **DRY CLI error exits**: Remaining ad-hoc `eprintln!("Error: ...")`/`std::process::exit(1)` blocks in `mimir/src/commands.rs` were routed through the shared `exit_with_error` helper.
 - **Shared CLI error helper exported**: `commands::exit_with_error` is now `pub` so `mimir/src/main.rs` can use it for the `ask` no-query guard, removing another standalone error-exit block.
 - **DRY init/main warnings and errors**: `mimir/src/init.rs` and `mimir/src/main.rs` now use shared `exit_with_error` and a local `warn_on_err` helper, removing duplicated `eprintln!`/`std::process::exit(1)` blocks for daemon startup and systemd activation.
 - **DRY config init**: `Config::init` and `Config::init_at` now share a single `Config::write_default_config` helper instead of duplicating the atomic default-config writing logic.
 - **DRY identity seeding auto-merge**: The accidental-duplicate auto-merge loop in `seed_identity_facts` was extracted into `auto_merge_accidental_duplicates`, flattening nested matches and removing duplicated warning formatting.
 - **DRY best-effort warning helper**: Added a shared `warn_err` helper in `mimir-server` and applied it to tool registration, tools-config loading, alias wiring, and auto-merge, removing repeated `if let Err(e) = ... { tracing::warn!(...) }` blocks.
 - **Single-lock session cache**: `ContextManager::ensure_session_exists` now acquires the `sessions` cache lock once and holds it across the database existence check, removing a redundant second lock acquisition.
 - **DRY HTTP client URL builder**: `MimirClient` now builds endpoint URLs through a private `url()` helper, removing repeated `format!("{}/...", self.base_url)` strings.
 - **DRY environment overrides**: `Config::apply_env_overrides_with` now uses a local `set_from_env!` macro, collapsing dozens of repeated `if let Some(v) = getenv(...) { ... }` blocks into declarative one-liners.
 - **Shared server types**: `CategoryResponse` and `CategoryDetailResponse` are now re-exported through `mimir_server::types` alongside other shared API types.
 - **DRY init error handling**: `mimir init` uses a shared `exit_with_error` helper instead of a one-off `eprintln!`/`exit(1)` block.

### Added
 
 - Unit tests for `SkillsPermissionsConfig` load/save round-trip and invalid TOML handling.
 - Path helper tests for `skills_dir`, `history_path`, and `personalities_dir`.
 - Unit tests for the new `MimirClient` category methods.
 - Unit tests for the `warn_err` best-effort warning helper.
 - Unit test for the `MimirClient::url` helper.
 
 ## [0.43.4] — 2026-06-14

### Fixed

- Replaced all hardcoded `"finish_retrieval"` strings in `RetrievalAgent` with the `FinishRetrievalTool::NAME` constant, removing a maintenance risk if the tool name changes.
- `RetrievalAgent` now executes non-`finish_retrieval` retrieval tool calls concurrently via `futures::future::join_all`, while still assembling tool result messages in the original call order.

## [0.43.3] — 2026-06-14

### Fixed

- `RetrieveContextTool::name()` now returns `Self::NAME` instead of a hardcoded string, keeping the tool name and registry constant in sync.
- `RetrievalAgent::merge_entity_facts` now upgrades an "Unknown" root-entity placeholder when a typed entity with the same name is merged, and skips adding an "Unknown" placeholder when a typed entity already exists. This eliminates duplicate entities across `kg_related` root-entity accumulation and typed results from `kg_query`/`kg_search`.

## [0.43.2] — 2026-06-14

### Fixed

- Added language tags to unlabelled fenced code blocks in `docs/kg-tools.md`, `docs/retrieval-agent.md`, and `docs/wiki/retrieval-agent.md` to satisfy markdownlint MD040.
- Cleaned up the malformed release summary header in `docs/wiki/what-works-now.md` so the version and implemented features are accurate.
- `RetrievalAgent` entity/fact deduplication now preserves full identity: entities are matched by `name` *and* `entity_type`, and facts are compared using all structural and lifecycle fields.
- `RetrieveContextTool` no longer logs the raw retrieval task; it logs only the task length to avoid exposing potentially sensitive user context.
- `retrieve_context` now uses the request-resolved LLM (including per-request model overrides) instead of the startup LLM in both blocking and streaming chat handlers.

## [0.43.1] — 2026-06-12

### Fixed

- RetrievalAgent now emits a tool-result message for `finish_retrieval` even when the LLM erroneously calls it alongside other tools, preventing an unbalanced conversation that could be rejected by the backend.
- `accumulate_kg_query` now parses `valid_from` and `valid_until` from `KgQueryTool` JSON output instead of discarding them as `None`.

## [0.43.0] — 2026-06-12

### Added

- **Agentic context retrieval** (Issue #128). The main LLM can now call `retrieve_context` to launch a dedicated RetrievalAgent. The agent runs an ephemeral, internal LLM session with only retrieval tools (`kg_query`, `kg_related`, `kg_search`, `search_conversation_history`), investigating the knowledge graph and conversation history for up to 25 rounds before returning a structured `RetrievedContext`. This enables multi-step, parallel research for complex questions (e.g. "What should I make for dinner with Mary, Bob, and Tom?").
- New `RetrievedContext`, `RetrievedEntity`, `RetrievedFact`, `RetrievedRelation`, and `ConversationSnippet` types in `mimir-knowledge/src/retrieval/types.rs`.
- `FinishRetrievalTool` — internal termination signal used by the RetrievalAgent to signal completion.
- SSE `event: tool_call_start` in the streaming chat handler, emitted before each tool execution to give users real-time visibility into Mimir's research phase.

### Changed

- Bumped workspace version to 0.43.0.

## [0.42.2] — 2026-06-12

### Fixed

- `init_schema` now only rebuilds the FTS5 index when `messages_fts` is newly created, eliminating unnecessary startup latency and I/O for large conversation histories.
- `seed_identity_facts` inserts identity facts before the alias/auto-merge block, ensuring the canonical entity always has at least as many facts as any qualifying duplicate and preventing `auto_merge_pair` from deleting it.
- Removed dead `escape_fts5` duplicate from `mimir-knowledge/src/queries/entity.rs`; all callers already use `mimir_core::fts5::escape_fts5`.

## [0.42.1] — 2026-06-12

### Fixed

- `seed_identity_facts` now auto-merges bare-name duplicate entities when the preferred name matches an existing entity with ≤2 facts. This resolves the stale-duplicate scenario where a short-name entity was created before the alias was wired up to the canonical entity.
- Added `KnowledgeGraph::count_entity_facts()` helper in `mimir-knowledge` for counting facts referencing an entity as subject or object.

## [0.42.0] — 2026-06-12

### Added

- Added `messages_fts` FTS5 virtual table for full-text search over conversation history (`mimir-core/src/context.rs`).
- Added `ContextManager::search_messages()` for BM25-ranked search with snippet extraction.
- Added `search_conversation_history` built-in tool (`mimir-core/src/tools/builtins/search_conversation_history.rs`).
- Extracted `escape_fts5` to `mimir-core/src/fts5.rs` for shared use across crates.

### Changed

- **Breaking (internal):** Migrated `sessions.id` from `TEXT` (UUID) to `INTEGER PRIMARY KEY AUTOINCREMENT` for faster lookups and smaller storage. All session IDs are now `i64` across the workspace.
- Removed `uuid` dependency from `mimir-core`.
- Incognito session IDs now use negative atomic `i64` counters instead of UUIDs.
- Axum routes now auto-reject non-numeric session IDs with `400 Bad Request` via `Path<i64>`.

### Fixed

- Updated all tests, benchmarks, and integration tests to use integer session IDs.
- Updated API types (`mimir-api-types`), server routes (`mimir-server`), client library (`mimir-client`), CLI (`mimir`), and documentation to reflect integer session IDs.

## [0.41.3] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (additional finding addressed):
  - Added `MIMIR_SCHEDULER_DEBOUNCE_SECONDS` and `MIMIR_SCHEDULER_COOLDOWN_SECONDS` environment variable overrides in `mimir-core/src/config.rs`, following the existing `apply_env_overrides_with` pattern.

## [0.41.2] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (5 findings addressed):
  - Removed unused `jq_for_opt` clone in `mimir-server/src/state.rs` optimisation job closure.
  - Added `DaemonJob::from_job_id()` helper to eliminate duplicated string-to-variant mapping in `mimir-core/src/scheduler.rs`.
  - Log SQL errors in `relationship_type_id` instead of silently swallowing them with `.ok()?`.
  - Clarified memory condensation documentation: separated 2500-character budget from top-N limit (500).
  - Corrected nightly-optimization wiki to state "last minute" instead of "last few minutes" to match the 60-second cooldown default.

## [0.41.1] — 2026-06-11

### Fixed

- `LlmWorkerPool` `in_flight` counter is now incremented and decremented around every job processed by workers. Previously the counter was always zero, causing the scheduler's idle gate to incorrectly allow background jobs while LLM requests were in flight.
- `BackgroundScheduler::submit()` now correctly deduplicates against jobs that are already *running*, not just pending. Prevents back-to-back execution when a submit arrives during an active run.
- `BackgroundScheduler::shutdown()` is now called during `AppState::shutdown()`, wiring the scheduler's private shutdown channel into the daemon's graceful teardown sequence. Prevents stale "Running" DB rows when the runtime drops mid-job.

## [0.41.0] — 2026-06-11

### Added

- Unified `BackgroundScheduler` in `mimir-core` that deduplicates, debounces, and gates all background jobs on user downtime and LLM idle state.
- `DaemonJob` typed enum replaces stringly-typed job IDs for `JobQueue::run_now` and `status`.
- Demand-driven memory condensation: `KnowledgeGraph` emits a `tokio::sync::Notify` on dirty; a listener submits `DaemonJob::MemoryCondensation` to the scheduler.
- Configurable `memory.condensation_top_n` (default 500) replaces hard-coded top-20 hash in condensation pipeline.
- `[scheduler]` config section with `debounce_seconds` (default 5) and `cooldown_seconds` (default 60).
- `LlmWorkerPool` tracks in-flight job count via `in_flight_count()`, exposed through `LlmBackend`.

### Changed

- Replaced fixed 30-second interval loop for auto-condensation with event-driven scheduler.
- `POST /memory/refresh` now uses `force_submit` to bypass scheduler gates.
- Nightly optimization callback now submits condensation through the scheduler instead of direct `run_now`.
- `JobQueue::list_jobs()` added for scheduled-job polling.

### Fixed

- `relationship_type_id` no longer uses `?` on `Result` inside `Option`-returning function (Rust 2024 edition compatibility).

## [0.40.7] — 2026-06-10

### Fixed

- Fact extraction now falls back to parsing the assistant's text content as JSON
  when the LLM does not emit a structured tool call. This resolves intermittent
  extraction failures with backends such as Ollama + Gemma that do not support
  `tool_choice`.
- The daemon guard spawns the background server in its own Unix process group,
  preventing Ctrl-C in the terminal from killing the daemon.
- `generate_and_install_service_file` now ensures config and data directories
  exist before writing the systemd unit, preventing NAMESPACE failures when
  `ReadWritePaths` references missing directories.

## [0.40.6] — 2026-06-10

### Fixed

- Addressed remaining CodeRabbit review feedback for PR #125:
  - Fixed CHANGELOG entry for uninstall.sh redirect typo to use literal characters.
  - Aligned test-only init_at() with production init() by ensuring cache directory exists.
  - Replaced silent unwrap_or((0,)) with explicit match on the fact-count query during auto-merge to avoid treating DB errors as zero facts.
  - Documented the auto-merge threshold (fact_count <= 2) in process_extracted_fact.
  - Optimized category validation in insert_facts_batch to query only referenced category IDs instead of the full table.
  - Tidied SQL formatting in get_facts_by_subject_and_predicate.
  - Documented the alias score adjustment (1.1) in entity search queries.
  - Removed unreachable Windows path checks from the Linux-only resolve_executable_path function.
  - Added defensive mimir substring check in uninstall.sh remove_dir before rm -rf.
  - Removed unused serial_test::serial imports in mimir-core tests.

## [0.40.5] — 2026-06-10

### Fixed

- Fixed fact-loss bug where multiple atemporal facts with the same subject and predicate but different objects (e.g. multiple hobbies) would incorrectly supersede each other, leaving only the last-inserted fact. The temporal overlap logic in `insert_fact_in_tx` now respects a `MULTI_VALUED_PREDICATES` allow-list (`hobby`, `likes`, `has_pets`, `has_sibling`, etc.) so that independent values for these predicates coexist instead of overwriting one another.

## [0.40.4] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (additional findings addressed):
  - Fixed typo in `scripts/uninstall.sh` where `error()` redirected with `&&2` instead of `>&2`.
  - Fixed `insert_facts_batch` atomicity by calling `ensure_relationship_type_in_tx` inside the batch transaction instead of autocommitting via `ensure_relationship_type`.
  - Moved `preferred_name` alias registration and auto-merge side effects in `process_extracted_fact` to after the dedup/corroboration check, preventing irreversible mutations on duplicate facts.
  - Aligned `generate_service_file` implementation with its docs and test by removing the unused `cache_dir` parameter and updating callers.

## [0.40.3] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (8 findings addressed):
  - Strengthened `normalize_predicate` to handle `name` → `has_name`, `nickname` → `preferred_name`, `favorite_food`/`color`/`colour` variants, and trimmed leading/trailing whitespace.
  - Expanded `LIST_PREDICATES` to include `has_pets`, `has_child`, `has_parent`, `has_sibling`, and `has_partner`.
  - Removed extra whitespace from the `remember` tool description.
  - `remember` tool output now includes actual error messages instead of just counts.
  - Replaced flaky `tokio::time::sleep(200ms)` in chat integration test with a deterministic polling loop and timeout.
  - `spawn_fact_extraction` now skips empty/whitespace-only messages.
  - Renamed `user_message_clone` to `user_message` in `chat_stream_handler` to clarify ownership.
  - Optimized `seed_identity_facts`: replaced full 1,000-fact scan with targeted predicate-specific queries; both identity inserts are now performed atomically via `KnowledgeGraph::insert_facts_batch`.

### Changed

- Added `relationship_type_id`, `get_facts_by_subject_and_predicate`, and `insert_facts_batch` to `KnowledgeGraph` API.

## [0.40.2] — 2026-06-10

### Fixed

- **Chat fact extraction wired up**: The fact-extraction pipeline (`mimir-knowledge/src/extract.rs`) was fully implemented but never triggered from chat. Both `/chat` and `/chat/stream` endpoints now spawn a background task after persisting the assistant response to extract facts from the user message. This fixes the long-standing issue where Mimir could query the knowledge graph but never write to it from conversation.
- **DRY refactor**: Extracted the duplicated extraction-spawning logic into `spawn_fact_extraction` in `mimir-server/src/routes/chat.rs`.

- **`remember` tool**: Registered `RememberTool` in the tool registry so the LLM can proactively write facts during conversation. The tool accepts structured `RememberOutput` and processes each fact through the same validation, dedup, confidence-assignment, and insertion pipeline used by background extraction.
- **System prompt updated**: The injected memory note now tells the LLM to use the `remember` tool whenever the user shares something worth saving.
- **Extraction prompt enriched**: Added detailed predicate standards (e.g., `studied_at` not `attended`, `hobby` not `hobbies`), explicit list-splitting instructions, and deduplication guidance to the fact extraction system prompt.
- **Predicate normalisation**: Rust-side `normalize_predicate` maps common LLM synonyms to canonical names (e.g., `attended` → `studied_at`, `hobbies` → `hobby`).
- **Comma-separated list splitting**: `split_list_objects` expands single facts with comma-separated values into multiple independent facts for allow-listed predicates (e.g., `hobby: "A, B, C"` → three separate `hobby` facts).

### Changed

- **Documentation**: Updated `docs/fact-extraction-pipeline.md`, `docs/chat-server.md`, and `docs/wiki/fact-extraction.md` to reflect that extraction is now live in the daemon.

## [0.40.1] — 2026-06-10

### Fixed

- **Personality prompts**: Removed references to a non-existent `memory` tool from `transparent`, `concise`, and `warm` presets. The LLM was instructed to use a tool that was not registered, causing `ToolError::NotFound("memory")` during conversations.
- **Identity seeding**: When the server starts, it now inserts `has_name` and `preferred_name` facts into the knowledge graph for the user entity (if not already present). This ensures Mimir can learn the user's identity through the existing memory condensation pipeline instead of relying on prompt injection.

## [0.40.0] — 2026-06-10

### Added

- **Issue #63**: Comprehensive testing suite for `mimir-knowledge`.
  - Inline unit tests for confidence model, clock, entity/fact models, and forget logic.
  - Temporal point-in-time DB integration test (`tests/temporal_point_in_time.rs`).
  - Criterion benchmark suite (`mimir-knowledge/benches/kg_benchmarks.rs`) with 10k-fact dataset covering entity resolution, FTS5, graph traversal, inference chain, and memory condensation.
  - `Clock::today()` and `MockClock::advance(Duration)` for deterministic temporal testing.

### Changed

- `MockClock::advance_seconds(i64)` replaced with `advance(&self, duration: Duration)`.


## [0.39.0] — 2026-06-10

### Added

- **Issue #61**: Full `mimir kb` CLI command suite (Phase A).
  - New commands: `kb query`, `kb show`, `kb edit`, `kb browse`, `kb profile`.
  - Existing commands (`kb audit`, `kb forget`, `kb restore`, `kb trash`) rewritten to go through the daemon via HTTP instead of opening SQLite directly.
  - All commands support `--json` for scripting output.
  - Human-readable output uses `tabled` for tables and `colored` for confidence color-coding (green >0.9, yellow 0.7–0.9, red <0.7).
  - Server routes added under `/kb/`: `query`, `facts/:id`, `facts/forget`, `browse`, `profile`, `audit`, `trash`, `trash/restore`.
  - Shared API types added to `mimir-api-types`: `FactQueryParams`, `FactDetailResponse`, `FactEditRequest`, `BrowseRequest`, `ProfileRequest`, `AuditQueryRequest`, `ForgetRequest`, `RestoreRequest`, `TrashListResponse`, and supporting row types.
  - New `update_fact` method in `mimir-knowledge` for structured field editing with transactional audit logging.
  - Server integration tests for all new routes.

### Changed

- CORS configuration now allows `PATCH` and `DELETE` methods.

## [0.38.0] — 2026-06-09

### Changed

- **Issue #112**: Switched chat context injection wording from `## Persistent Memory Context` to `Key facts I know about you:`.
  - Signals to the LLM that the injected memory is a curated subset, not an exhaustive record.
  - LLM should continue to use KG tools (`kg_query`, `kg_search`) for deeper or exhaustive queries.
- Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs` to use the new wording.
- Updated unit and integration tests in `mimir-core` to assert the new prompt text.

### Added

- Added server integration tests in `mimir-server/src/lib.rs`:
  - `test_chat_injects_kg_memory_into_system_prompt`: verifies blocking `/chat` injects KG condensed memory into the system prompt.
  - `test_chat_stream_injects_kg_memory_into_system_prompt`: verifies SSE `/chat/stream` injects KG condensed memory into the system prompt.


## [0.37.0] — 2026-06-08

### Removed

- **Issue #111**: Deleted the legacy `memory.md` file-backed memory system entirely.
  - Removed `mimir-core/src/memory/` directory (`MemoryManager`, `MemoryLoader`, `MemorySnapshot`).
  - Removed `MemoryTool` from `mimir-core/src/tools/builtins/`.
  - Removed `memory_manager` benchmark from `mimir-core`.
  - Cleaned stale `# path = "${CONFIG_DIR}/memory.md"` example comments from config TOML strings.

### Changed

- Memory is now exclusively knowledge-graph-backed via `mimir-knowledge`.
- `mimir-core` no longer exports a `memory` module; all memory access flows through `mimir-knowledge::KnowledgeGraph`.


## [0.36.0] — 2026-06-08

### Removed

- **Issue #110**: Removed all remaining file-based memory.md scaffolding.
  - memory.path and MIMIR_MEMORY_PATH env override removed from MemoryConfig.
  - MemoryTool unregistered from daemon and CLI tool list.
  - MemoryLoader::init() no longer called during mimir init.
  - AppState no longer carries memory_path or syncs memory.md on shutdown.
  - StatusResponse no longer includes memory_path.
  - mimir-core/src/paths.rs no longer exports memory_path().

### Changed

- mimir memory CLI and /memory server route now exclusively serve knowledge-graph-backed condensed memory.
- mimir status and chat REPL /status display no longer show the deprecated memory.md path.

### Added

- CLI parsing test for mimir memory --refresh flag.

### Documentation

- Updated docs/memory-system.md, docs/cli.md, docs/chat-server.md, docs/shutdown.md, docs/wiki/memory.md, docs/wiki/what-works-now.md, docs/wiki/cli-commands.md, docs/wiki/configuration.md, and docs/wiki/tools.md to remove memory.md references and describe the KG-backed system.

## [0.35.3] - 2026-06-08

### Fixed
- Fixed `sqlx::migrate!` not recognising `-- no-transaction` in migrations 031, 032, and 033 because the directive was preceded by comment headers. This caused those migrations to run inside transactions, which in turn caused `PRAGMA foreign_keys = OFF` to be ignored. Migration 033's `DROP TABLE relationship_types` then triggered an `ON DELETE CASCADE` that silently emptied `relationship_constraints`, breaking `test_predicate_validation`.


## [0.35.2] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback (CodeRabbit AI):
  - Removed duplicate 0.35.1 section from CHANGELOG.
  - Fixed oversize LLM output handling in memory condensation to use deterministic fallback instead of truncation, preventing underflow at `char_limit == 0`.
  - Recurring event output now uses the computed next occurrence date instead of the stored historical date.
  - Search failures during user entity resolution are now handled separately from "not found", preventing duplicate entity creation on transient errors.
  - Memory condensation job failures are now propagated to the job queue result instead of being silently swallowed.
  - Auto-trigger condensation loop is now skipped when no user entity is configured, preventing perpetual 30-second re-triggers.
  - `mimir init` now falls back to system identity when blank/whitespace input is provided.
  - `mimir memory --refresh` now surfaces server-side errors in the CLI output and exits with a non-zero status on failure.
  - Added client tests for `memory_refresh()` success and error paths.
  - Added server route tests for `/memory/refresh` non-loopback rejection, not-registered, and already-running cases.

## [0.35.1] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback:
  - Status endpoint now reads live condensed memory and upcoming section from the knowledge graph instead of the deprecated `memory.md` file.
  - `condensation_dirty` flag now automatically triggers the memory condensation job via a background watcher in the daemon.
  - Removed unused `whoami` dependency from `mimir-core`.
  - Removed dead `condensation_queued` field from `AppState`.
  - Centralised `recurrence_type_id` to `RecurrenceType` mapping via `TryFrom<i16>` in the enums module.
  - Chat system prompt builder now logs warnings when knowledge graph memory queries fail.
  - DRYed the SQL query in `build_memory_schema_with_opts` by constructing it once with a conditional predicate.
  - Fixed budget truncation loop so facts in `exclude_from_budget` buckets are still collected after the character budget is exhausted.

## [0.35.0] — 2026-06-07

### Added
- **Live Memory System (Issue #109)** — Replaced static `memory.md` with an event-driven, knowledge-graph-backed memory block.
  - Stable facts are condensed by the LLM and cached in `system_state.condensed_memory`.
  - Upcoming events (entity dates + temporal facts) are rendered fresh on every request.
  - Regeneration triggers: fact mutations, explicit `mimir memory --refresh`, and nightly optimization completion.
  - Pure formatting LLM prompt with deterministic fallback on failure or oversized output.
  - Sensitive facts are excluded from the LLM condensation pipeline.
- **Identity configuration** — `mimir init` now prompts for full name and preferred name, stored in `[identity]` config section.
- **User entity auto-resolution** — Daemon resolves the user entity from config at startup, creating it in the KG if missing.

### Changed
- `/memory` HTTP route now returns the live condensed memory block instead of `memory.md`.
- Chat system prompt now injects the live memory block from the knowledge graph.
- `build_memory_schema` supports `exclude_buckets` and `exclude_sensitive` options.
- `OptimizationRunner` now supports an `on_complete` callback for post-optimization hooks.

### Deprecated
- `memory.md` file-based memory is deprecated. `MemoryTool` writes are now logged as warnings.

## [0.33.2] - 2026-06-05

## [0.34.2] - 2026-06-07

### Fixed

- **Addressed PR #113 review feedback** (CodeRabbit AI review round 2):
  - Added serde default for `memory_priority_id` in `Fact` model to preserve legacy trash payload deserialization.
  - Replaced magic priority ID fallback (`3`) with semantic SQL lookup against `memory_priorities` table.
  - Fixed fire-and-forget centrality cache updates by making `bump_centrality` and `drop_centrality` async.
  - Eliminated TOCTOU race in `build_memory_schema` cache population with a read-then-populate pattern.
  - Replaced hardcoded category ID lists in `determine_bucket` with named constants.
  - Fixed potential UTF-8 panic in `truncate_fact` with char-aware truncation.
  - Reformatted SQL strings across `trash.rs` and `inference_tests.rs` for readability.
  - Updated documentation version references and corrected incomplete sentences.

## [0.34.1] - 2026-06-06

### Fixed

- **Review fixes for PR #108**: addressed 3 critical review findings in fact ranking engine.
  - Wired up `memory_priority_id` from `relationship_types.default_memory_priority_id` during fact insertion (`queries/fact.rs`, `extract.rs`, `models/fact.rs`).
  - Moved `drop_centrality` cache decrements to occur **after** `forget_fact` database transaction succeeds (`lib.rs`), preventing permanent cache drift on DB errors.
  - Fixed `truncate_fact` budget edge case (`queries/memory.rs`) so that when remaining budget is smaller than `subject + relationship + 3` overhead, `object_display` is correctly truncated to `…` instead of silently exceeding the budget.


### Fixed

- **Review fixes for PR #107**: addressed 10 CodeRabbit review findings across knowledge graph, server, and CLI.
  - `extract.rs` prompt now includes sub-categories with indentation so the LLM can pick specific IDs.
  - `lib.rs` fact insertion now validates category IDs before `INSERT OR IGNORE`, failing loudly on non-existent categories.
  - `queries/category.rs` replaced magic `NOT IN (5, 6)` with bound `FactStatus::Superseded` / `Forgotten` parameters.
  - `kg_expand_catalogue.rs` now queries real `fact_count` for each child category instead of hard-coding `0`.
  - `integration_tests.rs` merge assertion tightened with `object_id` filter to avoid false positives.
  - `error.rs` no longer leaks raw internal KG error strings in `500` HTTP responses.
  - `lib.rs` (server) tool-registry tests now assert `expand_catalogue` and `get_facts_in_catalogue` are exported.
  - `chat.rs` only fetches the catalogue DB when a new session or incognito turn starts, avoiding hot-path latency.
  - `cli.rs` `category add` now exposes `--memory-weight` to match the server API.
  - `kb.rs` JSON decode failures are no longer swallowed with `unwrap_or_default()`; they now surface as fatal CLI errors.

## [0.33.1] - 2026-06-05

### Fixed

- **P2**: `get_facts_matching_all_categories` now deduplicates input category IDs before querying, preventing empty results when duplicate IDs are passed.
- **P3**: Removed unused `client` variable in `mimir/src/kb.rs` (`handle_kb_category`).
- **P3**: Simplified redundant closures in `mimir-server/src/routes/kb_categories.rs` (5 instances of `.map_err(|e| error::knowledge_error(e))?` → `.map_err(error::knowledge_error)?`).

## [0.32.2] - 2026-06-05

### Fixed

- **Review fixes for PR #92**: addressed 14 CodeRabbit review findings across job queue, optimization pipeline, documentation, and daemon routes.

## [0.32.1] - 2026-06-04

### Fixed

- **P1**: `optimization_pass_runs` now linked to parent `optimization_runs` via foreign key `run_id`. `OptimizationRunner` inserts a parent row at pipeline start and updates it on completion or failure. Failed passes are recorded with error text instead of being silently omitted.
- **P1**: `DailySchedule::next_after` now converts the stored naive local time to UTC using `chrono::Local`, fixing scheduling for non-UTC timezones.
- **P1**: `chat_stream_handler` now calls `state.record_user_activity()`, ensuring SSE stream interactions update `last_user_activity` and prevent premature job yielding.
- **P2**: `JobQueue::run_now` now rejects concurrent executions of the same job by checking for an existing `Running` row in `job_runs`.
- **P2**: `semantic_dedup` candidate query now includes `ORDER BY a.id, b.id` for deterministic candidate selection.
- **P2**: `semantic_dedup` now uses a structured LLM tool schema (`evaluate_dedup_candidates`) instead of relying on raw JSON parsing from a plain-text prompt.


## [0.32.0] - 2026-06-04

### Added

- JobQueue and nightly optimization pipeline (issue #58):
  - New `mimir-core::job_queue` with durable job definitions, runs, scheduling, and manual triggers.
  - `JobQueue` persisted in `jobs.db` with `Job`, `JobPriority`, `JobStatus`, `JobRunStatus`, `DailySchedule`, `JobContext`, and `JobRunSummary` public types.
  - Config support for `[knowledge.optimization]` defaults: `cpu_cores = 1`, `nice_level = 10`, `timeout_minutes = 120`, `schedule_time = "02:00"`.
  - Daemon tracks user activity in `AppState`; chat routes record interaction time.
  - System jobs yield between pass boundaries when user activity is inside the 5-minute idle window.
  - Daemon routes: `GET /kb/optimization/status` and `POST /kb/optimization/run-now` (loopback-only for run-now).
  - CLI commands: `mimir kb optimization --status` and `mimir kb optimization --run-now`.
  - Refactored `mimir-knowledge/src/optimization` into pass modules with 10 nightly passes (7 core optimization passes plus 3 cleanup steps):
    - Pass 1: deterministic dedup (exact triple merge).
    - Pass 1b: semantic dedup via LLM structured JSON; auto-merge >= 0.9 confidence, queue uncertain pairs.
    - Pass 2: contradiction resolution.
    - Pass 3: inference chain re-evaluation.
    - Pass 4: confidence recalculation.
    - Pass 5: dormant cleanup (old disputed non-user facts).
    - Pass 6: pattern consolidation stub.
    - Pass 7: compaction (FTS rebuild, ANALYZE, VACUUM).
    - Plus: pending confirmation cleanup (7-day TTL) and trash cleanup.
  - Pre-pass backup with `VACUUM INTO` to `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db` with counter suffix for collisions.
  - Per-pass run recording in `optimization_pass_runs` table.
  - Integration tests for daemon routes and CLI client methods.

### Changed

- `run_nightly_optimization` compatibility wrapper now delegates to `OptimizationRunner::run_all`.
- `cascade_inner` in `confidence.rs` future is now `Send`-safe.

## [0.31.1] - 2026-06-04

### Fixed

- **P1**: `restore_all` now maps both child and parent IDs through `id_map` when rebuilding `fact_dependencies`, preventing FK violations on restored facts.
- **P1**: `restore_fact` now marks the trash row as restored, preventing duplicate restores and stale trash listings.
- **P1**: `hard_delete_all_facts` correctly reports the number of forgotten facts via `rows_affected()` instead of querying the now-empty table.
- **P1**: `create_backup` escapes single quotes in the backup path before interpolating into `VACUUM INTO`, preventing SQL injection/breakage from `XDG_DATA_HOME` paths containing apostrophes.
- **P2**: Restoration audit log now references the newly generated fact ID instead of the original deleted ID.

## [0.31.0] - 2026-06-04

### Added

- Phase 2: Forgetting system -- trash, cascade forget, restore, bulk operations (#57)
  - Bulk forget by predicate, entity, source, time range, and full reset.
  - Trash bin with 30-day expiry, restoration, and automatic nightly cleanup.
  - Cascade forget for inferred facts: orphan removal and confidence recalculation.
  - Bulk safeguards: >100 facts requires --yes, sensitive predicates require --confirm-sensitive, full reset requires typing DELETE EVERYTHING.
  - Full reset creates a timestamped SQLite backup via VACUUM INTO.
  - New CLI commands: mimir kb forget, mimir kb restore, mimir kb trash.
  - Extended TrashPayload with dependency chains so restored facts rebuild parent links.
  - Sensitive predicate flag (sensitive BOOLEAN) on predicates table with seeded defaults for medical/financial terms.


## [0.30.1] - 2026-06-04

### Fixed

- **P1**: `kg_query` and `kg_related` no longer mutate the database via `ensure_predicate` during read-only tool calls. Both now use the new read-only `get_predicate_id` lookup; missing predicates return empty results instead of silently inserting rows.
- **P2**: `AppState` knowledge graph and context database fallbacks now propagate `PathsError` instead of using a broken tilde (`~`) literal path.
- **P3**: `kg_search` now returns an explicit invalid-arguments error when an unrecognized `entity_type` is supplied, rather than silently ignoring the filter.

## [0.30.0] - 2026-06-04

### Added

- Phase 2: Knowledge Graph LLM tools — `kg_query`, `kg_related`, `kg_search` (#56)
  - Database migration `028_add_performance_indexes.sql` for tool query performance.
  - Query layer: `search_entities`, `traverse_graph`, `get_facts_by_subject_filtered`, `get_entity_names`.
  - Tool implementations in `mimir-knowledge/src/tools/` implementing `mimir_core::Tool`.
  - Server integration: `AppState` initialises `KnowledgeGraph` and registers all three tools.
  - Input sanitisation, FTS5 injection defence, and SQL-level exclusion of pending/superseded/forgotten facts.
  - Comprehensive unit and integration tests.


## [0.29.2] - 2026-06-03

### Fixed

- `mimir-knowledge/src/optimization/mod.rs`: `cleanup_stale_pending_confirmations` now deletes `fact_dependencies` rows before deleting the fact and wraps each deletion in a transaction, avoiding `ON DELETE RESTRICT` violations and ensuring atomic DB/cache state.

## [0.29.1] - 2026-06-03

### Fixed

- `mimir-knowledge/src/extract.rs`:
  - `confirm_fact` now cascades inferred facts instead of discarding them (P1).
  - `find_existing_fact` dedup query now matches pending-confirmation facts, preventing duplicate sensitive extractions (P1).
  - `handle_correction` retrospective loop is now atomic: all overlapping facts are marked `Corrected` and soft-deleted in a single transaction before child evaluation (P2).
- `mimir-knowledge/tests/extraction_test.rs`: corrected misleading comment in `test_casual_extraction` (P3).

## [0.29.0] - 2026-06-03

### Added

- Fact extraction pipeline (issue #55):
  - `mimir-knowledge/src/extract.rs`: full LLM → Rust validation → entity resolution → confidence assignment → sensitive confirmation → fact insertion pipeline.
  - LLM tool `remember`: structured schema for extracting subject-predicate-object triples with classification (Explicit / Casual / Correction), temporal bounds, and sensitivity flags.
  - Entity resolution: names matched via exact → alias → FTS5 fuzzy; new entities auto-created with LLM-provided type.
  - Confidence assignment: classification maps to `SourceType` → `confidence::initial()`; LLM hints are ignored.
  - Correction handling:
    - Temporal: `correction_scope` as ISO-8601 datetime closes the sole open-ended predecessor.
    - Retrospective: `correction_scope = "always"` marks overlapping facts as `Corrected`, moves them to trash, and inserts the new fact.
  - Sensitive fact confirmation flow:
    - Sensitive facts inserted as `Disputed` with `pending_confirmation = TRUE`.
    - In-memory `HashSet<i32>` cache rebuilt from DB on startup.
    - `confirm_fact`: flips to `Active`, confidence `1.0`, triggers inference.
    - `reject_fact`: hard-deletes with `Rejected` audit entry.
  - Corroboration stub for issue #79: duplicate facts returned in `ExtractionOutcome::corroborated` without insertion.
  - 11 integration tests covering explicit, casual, entity resolution, temporal/retrospective correction, sensitive confirmation/rejection, multiple facts, empty extraction, and invalid LLM output.

### Changed

- `facts` table: added `pending_confirmation BOOLEAN NOT NULL DEFAULT FALSE` (migration 026).
- `change_types` table: added `rejected` (migration 027).
- `Fact` model: added `pending_confirmation` field.
- `ChangeType` enum: added `Rejected = 8`.
- `ranges_overlap` in `queries/fact.rs`: made `pub` for reuse in extraction pipeline.


## [0.28.1] - 2026-06-02

### Fixed

- Review feedback on inference engine (issue #54):
  - `CHANGELOG.md`: reordered 0.28.0 section to top with markdownlint blank lines.
  - `docs/inference-engine.md`: explicit facts are detected by `!inferred` rather than `confidence == 1.0`.
  - `mimir-knowledge/src/inference/mod.rs`: streaming evaluation for `evaluate_batch` (pending — rule loop still materialises; moved to follow-up).
  - `contradiction.rs`: explicitness uses `!inferred`; status updates wrapped in atomic transactions via `set_status_tx`.
  - `threshold.rs`: DB errors propagated instead of `unwrap_or(0)`; stale preferences deleted when source fact missing; duplicate `StatusChange` audit entries deduplicated within 24h.
  - `transitivity.rs`: trigger queries include `FactStatus::Inferred`; inferred facts use temporal intersection of parent windows.
  - `lib.rs`: `ensure_predicate` insert is atomic with `ON CONFLICT`.
  - `NewFact`: removed `Default` impl; added `NewFact::new(subject_id, predicate)` constructor.
  - `optimization/mod.rs`: confidence cascade uses unlimited depth (`None`); operational errors propagated instead of swallowed.
  - Tests: predicate name roundtrip restored; unknown predicate test uses absent ID; contradiction relation type asserted; cycle-safety contract replaces brittle exact count.

## [0.28.0] - 2026-06-02

### Added

- Inference engine core with `InferenceRule` trait, `RuleEngine`, and `CascadeContext` for cycle-safe unbounded cascades.
- Transitivity rule: `visited`/`is_in` + `is_in` chain → inferred transitive facts with depth-tracked confidence.
- Contradiction rule: real-time `Disputed` status + bidirectional `Contradicts` edges; nightly batch auto-resolves explicit > inferred disputes.
- Threshold rule: 3+ `rejected_action` facts → `General` preference upsert; nightly re-count warns if threshold drops.
- `PredicateRegistry` with `ensure_predicate` and `predicate_name` for unlimited extensible predicates backed by the DB.
- Migrations 024 (Contradicts relation type) and 025 (rejected_action predicate).
- Nightly optimization orchestrator (`run_nightly_optimization`) wiring contradiction resolution, confidence propagation, and inference re-evaluation.
- Integration tests for transitivity, contradiction, threshold, cascade, and cycle safety.

### Changed

- Removed compile-time `Predicate` enum; `NewFact.predicate` is now a `String` resolved at runtime.
- `Fact::predicate()` removed; callers use `kg.predicate_name(fact.predicate_id)`.
- `KnowledgeGraph::insert_fact` automatically runs inference rules and cascades inferred facts.
- `NewFact` extended with `inferred`, `inference_depth`, `confidence`, and `parent_fact_ids` fields.

### Documentation

- Added `docs/inference-engine.md` with architecture, rule descriptions, confidence formulas, and cascade behavior.
- Added `docs/wiki/inference-rules.md` with user-facing examples and best practices.

## 0.27.1 (2026-06-02)

> Next-day hotfix release for 0.27.0.

### Fixed

- Atomic upsert: delete and insert now happen in a single transaction, preventing data loss on crash between commit and insert.
- Contextual lookup now correctly falls back to the default (zero-context) preference when no contexts match, instead of ranking by confidence.
- `preference_sources` now binds `extracted_at` explicitly for deterministic timestamps.
- `preference_audit_log` stores `NULL` for the `reason` column on creation events instead of an empty string.
- `get_preference` eliminates N+1 queries by fetching all contexts in a single query.
- Uniqueness checks in `insert_preference` and `upsert_preference` no longer clone the full context `HashSet`.
- Confidence validation now happens before acquiring a database write lock.
- Migration 023 now seeds `predicate_constraints` for `HasPreference` so `validate_predicate` does not fail.

## 0.27.2 (2026-06-02)

### Fixed

- Review feedback on preference system (issue #53):
  - `source_fact_id` is now nullable in `preferences` table and Rust types (`Option<i32>`).
  - Explicit preferences (`overridden_by_user = true`) now require `confidence = 1.0` at validation time.
  - `UpsertAction::Overwritten` now updates the existing preference row in-place instead of deleting and re-inserting, preserving the audit trail.
  - Clarified that the 11 seeded predicates in `predicate_constraints` are the complete set.

## 0.27.0 (2026-06-01)

### Added

- Preference system refactor (issue #53): behavioural index over the fact graph with contextual lookup and conflict resolution.
- New `Predicate::HasPreference = 11` seeded in `predicates` table.
- New lookup tables (re-seeded in migration 023):
  - `preference_categories`: 7 variants — CalendarBehavior, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, General.
  - `preference_source_types`: 3 variants — Interaction, Fact, UserEdit.
- New `PreferenceCategory` and `PreferenceSourceType` enums with `#[repr(i16)]` and `sqlx::Type`.
- New schema (migration 023):
  - `preferences` with `source_fact_id NOT NULL REFERENCES facts(id)`.
  - `preference_contexts` — normalized context conditions, no JSON.
  - `preference_sources` — provenance with `(preference_id, source_type_id, source_id)` unique constraint.
  - `preference_audit_log` — immutable history without FK to `preferences` (preserves history after deletion).
- Contextual lookup API: `get_preference(entity_id, key, query_context)` ranks by match count, confidence, and recency.
- Upsert API with conflict resolution:
  - Explicit overrides inferred.
  - Higher-confidence inferred wins.
  - Same confidence keeps existing.
  - `overridden_by_user = true` blocks inferred overwrites.
- Full audit logging on preference creation and overwrite.
- Source tracking for every preference.
- FK enforcement: non-existent `source_fact_id` is rejected.
- Comprehensive test suite in `mimir-knowledge/tests/preference_tests.rs` (15 tests).
- Technical documentation: `docs/preference-system.md`.
- User-facing documentation: `docs/wiki/preferences.md`.

### Changed

- **Breaking schema change:** old `preferences` and `preference_sources` tables dropped and recreated. No data migration attempted.

## 0.26.0 (2026-06-01)

### Added

- New built-in tool `get_weather` using wttr.in.
  - Fetches current conditions for any location (city name, airport code, or coordinates).
  - Returns structured JSON: temperature (°C/°F), feels-like, description, humidity, wind, UV index, visibility, and pressure.
  - Configurable base URL for testing (`GetWeatherTool::with_base_url`).

## 0.25.1 (2026-06-01)

### Fixed

- `get_active_facts_at` restored missing `AND fact_status_id = ?` filter so it again returns only active facts.
- `query_audit_log` switched from INNER JOINs to LEFT JOINs on `facts`, `entities`, and `predicates`, ensuring audit history remains visible after a fact is forgotten (hard-deleted).
- `mimir kb audit` now validates `--from` and `--to` datetime strings and exits with an error instead of silently ignoring malformed input.

## 0.25.0 (2026-06-01)

### Added

- Provenance audit refactor (issue #52): typed `change_type` and `changed_by` lookup tables with integer IDs.
- New lookup tables: `extraction_methods` (5 variants), `change_types` (7 variants), `changed_by_types` (4 variants).
- New `ExtractionMethod`, `ChangeType`, and `ChangedBy` enums with `#[repr(i16)]` and `sqlx::Type`.
- `mimir kb audit` CLI command for querying the fact audit log directly from the local SQLite database.
- `query_audit_log` API with filters: entity name, predicate name, datetime range, and change type.
- `add_source_to_fact` API for adding corroborating sources to an existing fact.
- `sources` unique constraint: `(fact_id, source_type_id, connector_id, raw_reference)`.
- Audit entries are now column-only JSON snapshots (e.g. `{"valid_until": ...}`) instead of full fact snapshots.

### Changed

- **Breaking schema change:** `source_types` remapped to 6 canonical variants: `UserEdit(1)`, `Connector(2)`, `Inference(3)`, `Interaction(4)`, `Import(5)`, `System(6)`. Old `Email`/`Calendar`/`Photo`/`Message` variants mapped to `Connector`; `CasualMention` mapped to `Interaction`.
- `fact_audit_log` recreated with `change_type_id`, `changed_by_id`, `reason`, and `changed_at` columns. Old action/performer strings migrated via best-effort mapping.
- `sources` recreated with `extraction_method_id INTEGER REFERENCES extraction_methods(id)`.
- `NewFact` expanded with `connector_id`, `connector_type`, `raw_reference`, and `extraction_method` fields.
- `update_fact_valid_until`, `update_fact_status`, and `forget_fact` now accept `ChangedBy` parameter.
- `forget.rs` deletes **all** `fact_dependencies` rows where the forgotten fact is parent or child (not just `InferredFrom`).
- Confidence cascade now writes `confidence_change` audit entries on child recalculation.

### Fixed

- Prevent duplicate edges when an already-superseded fact is superseded again by a third explicit fact.
- Correct `children` and `remaining_parents` queries in `forget.rs` after removal of relation_type filter from the DELETE query.

## 0.24.3 (2026-05-31)

### Added

- Structural confidence model (issue #51): confidence derived entirely from graph structure, zero LLM involvement, zero time-based decay.
- New `SourceType` variants: `CasualMention`, `Import`, `System`.
- New `ConnectorType` enum with SQLite lookup table and reliability tracking.
- `inference_confidence` formula: signed parent sum × chain penalty (0.8^depth) × breadth factor.
- `inference_depth` and `stale_confidence` columns on `facts` table.
- `is_positive` column on `fact_dependencies` for signed parent contributions.
- Per-connector reliability scores with feedback loop (`adjust_connector_reliability`).
- Eager bounded confidence cascade on parent removal.

### Changed

- `NewFact` no longer accepts caller-provided `confidence`; confidence is now computed in Rust (internal change; not public API).
- Connector-type source facts now use per-connector reliability scores instead of flat 0.80.
- Initial confidence values: `UserEdit`/`System` = 1.0, `CasualMention` = 0.30, `Import` = 0.80.

### Fixed

- Updated all test assertions and raw SQL to match new schema columns.

## 0.24.4 (2026-05-31)

### Fixed

- Build failure in `mimir-client`: replaced unsupported `reqwest` feature `rustls-tls-ring` with `rustls-native-certs` to align with `reqwest` 0.13 feature flags and `mimir-core` crate configuration.

### Documentation

- Added `docs/wiki/what-works-now.md`: comprehensive user-facing overview of all working features, current limitations, known bugs, and roadmap context.

## [0.33.0] - 2026-06-05

### Added

- **Category taxonomy system** (Dewey Decimal-style):
  - New `categories` table with hierarchical parent-child relationships.
  - `fact_categories` junction table allowing facts to belong to multiple categories.
  - Comprehensive seed taxonomy covering Identity (100), Food & Drink (200), Health (300), Relationships (400), Work (500), Home (600), Entertainment (700), Travel (800), and Schedule (900) with 2-3 levels of depth.
  - New KG tools: `expand_catalogue` and `get_facts_in_catalogue` for LLM-driven category browsing and fact retrieval.
  - System prompt injection of top-level catalogue so the LLM knows what knowledge domains exist.
  - CLI commands: `mimir kb category list`, `show`, `add`, `delete`.
  - Server routes: `GET /kb/categories`, `GET /kb/categories/{id}`, `POST /kb/categories`, `DELETE /kb/categories/{id}`.

- **Extraction pipeline category assignment**:
  - LLM suggests 1–3 category IDs per extracted fact via the `remember` tool.
  - Rust validates all suggested IDs against the database before insertion.

### Changed

- **Renamed `predicates` → `relationship_types`** and `predicate_constraints` → `relationship_constraints` across the entire codebase (DB schema, models, queries, tools, inference rules, tests).
- Updated all SQL queries, indexes, and foreign keys to use `relationship_type_id`.
- Updated `MemoryManager` and system prompt integration to read from the knowledge graph catalogue.

### Migration

- Migration `031_category_taxonomy_and_rename_predicates.sql` performs the rename and seeds the full category taxonomy.

## [0.34.0] - 2026-06-06

### Added

- **Issue #108**: Fact Ranking & Selection Engine (`mimir-knowledge`).
  - Introduced `memory_priorities` lookup table (Critical, High, Normal, Low) and `memory_priority_id` on `facts`.
  - Added `default_memory_priority_id` to `relationship_types` for automatic priority assignment at insertion.
  - Implemented scoring formula: `confidence × category.memory_weight × temporal_boost × priority_boost × centrality_boost`.
  - Temporal boost: `10.0 / sqrt(max(days, 0.5))` for future-dated facts (upcoming events, birthdays).
  - Centrality boost: entity connection count with in-memory `HashMap` cache, incrementally updated on mutation.
  - Budget fill algorithm: identity facts first (~200-char soft reservation), then greedy score-based fill to 2500-char limit.
  - Structured buckets: `identity`, `relationships`, `preferences`, `upcoming`, `general`.
  - Deterministic fallback renderer in Rust for when LLM condensation is unavailable.
  - `system_state` read/write queries for cached `condensed_memory`.
  - Unit and integration tests covering scoring, temporal boost, budget fill, renderer, and centrality cache.

## [0.38.1] — 2026-06-09

### Added

- **Issue #60**: Added explicit non-exhaustive note to context-injected system prompt.
  - When condensed memory is present, the system prompt now appends: "Note: This is not an exhaustive list. Use kg_query, kg_related, or kg_search tools if you need more information."
  - Signals the LLM that the injected memory is a curated subset, prompting tool use for deeper queries.
  - Completes the Layer 2 context injection design from Phase 2 Knowledge Graph architecture.
  - Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs`.
  - Updated unit tests, integration tests, and server integration tests to assert the note is present.
  - Updated documentation in `docs/personality-system.md`, `docs/wiki/personality.md`, and `docs/wiki/what-works-now.md`.
