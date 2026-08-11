# OAuth Client & Token Refresh (mimir-connectors)

> **Phase:** 3 — Connectors
> **Issue:** #240 (oauth2/reqwest reconciliation), #205 (PKCE loopback flow)
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md` §4
> **Landed in:** v0.96.0 (refresh), v0.97.0 (PKCE flow)

## Purpose

One shared OAuth 2.0 path for every connector that authenticates with OAuth: token refresh (Calendar C3 / #197, Email C5 / #199) and the interactive PKCE authorization-code login from A4 / #205. The implementation uses the vetted `oauth2` crate (5.0.0) with `default-features = false` and a custom HTTP adapter over the workspace's single reqwest 0.13 client.

## Why the adapter

`oauth2` 5.0.0's optional `reqwest` feature pins reqwest 0.12, which would duplicate the workspace's reqwest 0.13 HTTP/TLS stack if the crate were enabled with its default features. Because `oauth2`'s `HttpRequest`/`HttpResponse` are plain `http` 1.x types (the same version reqwest 0.13 uses), the crate's `AsyncHttpClient` trait can be implemented directly over the workspace client — the same pattern as `oauth2`'s own `reqwest_client.rs`, with no reqwest 0.12 in the tree. The workspace therefore keeps exactly one reqwest major while still using the vetted protocol code (PKCE S256, authorization-code and refresh grants) rather than hand-rolling security-sensitive OAuth logic.

## Module layout

- `src/oauth/mod.rs` — module docs, public surface (`OAuthHttpClient`, `PkceFlowConfig`, `run_pkce_flow`, `DEFAULT_FLOW_TIMEOUT`), internal re-exports.
- `src/oauth/http_client.rs` — the `OAuthHttpClient` newtype: `OAuthHttpClient::new()` builds a hardened reqwest 0.13 client (30 s timeout, `redirect::Policy::none`), `OAuthHttpClient::from_client(...)` wraps an injected client for tests, and the `AsyncHttpClient<'c>` impl converts `http::Request<Vec<u8>>` → reqwest request → `http::Response<Vec<u8>>`.
- `src/oauth/refresh.rs` — the refresh grant (`refresh_token`), the shared "resolve a live access token" helper (`resolve_access_token`), the bundle conversion (`into_bundle`), the HTTPS/loopback endpoint gate (`validate_token_endpoint`), and the secret-hygiene error mapping (`map_token_error`).
- `src/oauth/pkce.rs` — the interactive PKCE authorization-code flow (`run_pkce_flow`, A4 / #205): binds an ephemeral loopback listener on `127.0.0.1:0`, opens the provider's authorize URL (S256 challenge + CSRF state), receives the redirect, validates the state, exchanges the code via `request_async(&OAuthHttpClient)`, and returns the `SecretBundle::OAuth` for the caller to POST to the daemon's token-ingest route. The daemon never runs a transient HTTP server.

## Security properties

- **HTTPS-only token endpoints.** `refresh_token` and `run_pkce_flow` reject non-HTTPS endpoints before any credential is posted. Loopback HTTP (`localhost`, any `127.0.0.0/8`, `::1`) is permitted — it is Mimir's local trust boundary, the same model as the home-directory secret store. The host is parsed as a real `IpAddr`, so look-alike DNS names like `127.0.0.1.evil.com` are rejected.
- **Redirects disabled.** The OAuth client never follows redirects, so a compromised or malicious token endpoint cannot bounce a credential POST (refresh grant and code exchange) to an attacker-controlled host. This follows the `oauth2` crate's own SSRF guidance and fixes a gap in the pre-#240 hand-rolled refresh, which used the default redirect-following client.
- **Loopback-only callback listener.** The PKCE flow binds `127.0.0.1` (never `0.0.0.0`), so no remote host can race the callback and steal the authorization code. The callback request is read with an 8 KiB cap so a hostile local process cannot force a large allocation.
- **CSRF state validation.** The callback's `state` must match the generated `CsrfToken` or the flow aborts without exchanging. A browser's `/favicon.ico` probe is ignored rather than treated as a callback.
- **Secret hygiene.** Provider error payloads routinely echo request parameters (the refresh token or client secret). Error strings report only the HTTP-level outcome plus the parsed `error`/`error_description` fields (truncated to 256 bytes), never the raw response body — `RequestTokenError::Parse`'s body payload is deliberately dropped. This preserves the promise of the pre-#240 hand-rolled helper.
- **Diagnosable network failures.** oauth2's `HttpClientError::Reqwest` display is the constant `client error` (the inner error is not part of the format string), so the adapter formats the inner reqwest error (DNS / timeout / TLS / connection) into `ConnectorError::Network` — the actual cause reaches logs and `last_error`, and contains no secret material (URL + error kind).
- **Hostile `expires_in` saturates.** A token endpoint returning an absurd `expires_in` (beyond chrono's ±262143-year range) saturates at `DateTime::<Utc>::MAX_UTC` instead of panicking the refresh path (`DateTime + TimeDelta` overflows on such values).
- **Client credentials in the body.** `AuthType::RequestBody` sends `client_id`/`client_secret` as form fields (not HTTP Basic), matching the providers Mimir targets and the pre-#240 behaviour exactly.

## Feature gating

The `oauth` feature (default off) gates `oauth2` + `http` + `url` and the `oauth` module. It is enabled by the `calendar` and `gmail` backend features (both use refresh) and by the `mimir` binary (the CLI PKCE flow, A4 / #205). `--no-default-features` builds of `mimir-connectors` never compile the module.

## Usage

- **Refresh:** both connectors hold an `OAuthHttpClient` (`CalendarConnector::oauth_http`, `EmailConnector::oauth_http`) and their `resolve_auth` OAuth arms call `oauth::resolve_access_token(&oauth_http, token_endpoint, client_id, client_secret, scopes, bundle)`, which returns the live access token plus the refreshed bundle to persist (refresh-token rotation is retained — a response without a new `refresh_token` keeps the stored one).
- **PKCE (A4 / #205):** the CLI flow (`mimir/src/connector/oauth.rs`) extracts a `PkceFlowConfig` from the merged connector config (`auth.kind=oauth`), calls `run_pkce_flow(&config, &http, &open_in_browser, DEFAULT_FLOW_TIMEOUT)`, and POSTs the resulting `SecretBundle::OAuth` to the daemon's token-ingest route (`POST /connectors/{id}/tokens` via `mimir-client::connector_tokens`). The authorize URL is printed before the browser is opened, so headless/SSH sessions can complete the login manually; a browser-open failure is non-fatal. The flow runs before the instance is registered in `add`, so a canceled flow exits with nothing created.

## Dependency notes

oauth2 5.0.0's unconditional dependencies are already in the workspace tree (sha2 0.10, base64, serde_path_to_error, thiserror 1.x, http 1.x, url) except `rand 0.8` — a third rand line alongside 0.9 (governor) and 0.10 (transitive). It is small, ubiquitous, and required by oauth2's PKCE verifier generation. No reqwest-0.13-compatible `oauth2` release exists (latest is still 5.0.0 as of 2026-08-11), so the "wait for an upgrade" option is struck from the deps ledger. The `mimir` binary additionally depends on `webbrowser` 1.2.4 (cross-platform browser opening, MIT/Apache-2.0) and `mimir-connectors` declares `url` 2.x under the `oauth` feature (used by the PKCE callback parser).
