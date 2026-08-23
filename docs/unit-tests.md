# Unit Tests

This document covers the inline (`#[cfg(test)]`) unit-test coverage across the Mimir workspace. Integration tests live under each crate's `tests/` directory and are documented in `docs/integration-tests.md`.

## Scope of the `tests-and-benchmarks` pass

A workspace-wide pass expanded inline unit-test coverage for pure helpers, wire types, and response mapping that were previously only exercised indirectly (or not at all). All additions are deterministic and parallel-safe; none mutate process-global state (no `set_var`/`remove_var`).

## `mimir-api-types` (`src/lib.rs`)

63 tests (up from 12). Added a `roundtrip_tests!` macro that asserts both the populated and sparse (all-`None`) forms round-trip, and that `skip_serializing_if` fields are omitted from the sparse JSON. The sparse-field check parses the serialised JSON into a `serde_json::Map` and asserts key absence with `contains_key` (not a substring search, which could match value text). Covers every KG wire type: `FactQueryParams`, `FactRow`, `FactQueryResponse`, `SourceRow`, `DependencyRow`, `AuditRow`, `FactDetailResponse`, `FactEditRequest`, `FactEditResponse`, `BrowseRequest`, `BrowseEdge`/`BrowseResponse`, `CategoryResponse`/`CategoryDetailResponse`, `ProfileRequest`/`ProfileResponse`, `AuditQueryRequest`/`AuditQueryResponse`, `ForgetRequest`/`ForgetResponse`, `RestoreRequest`/`RestoreResponse`, `TrashRow`/`TrashListResponse`, `OptimizationRunSummary`/`OptimizationStatusResponse`/`OptimizationRunNowResponse`, `PendingFactRow`/`PendingListResponse`, `ConfirmFactResponse`, `RejectFactRequest`, plus `StreamItem` variant equality.

## `mimir-client` (`src/lib.rs`)

74 tests (up from ~24). Added pure unit tests for the SSE parser primitives (`find_double_newline`, `parse_sse_event` — text/usage/tool_call/session_id/ error/empty/multiline/invalid-UTF-8) and wiremock-backed integration tests for every previously-uncovered `MimirClient` method: `kb_optimization_status`, `kb_optimization_run_now`, `kb_query/show/edit/browse/profile/audit/forget/restore/trash/trash_empty/pending/confirm/reject`, `stop` (success/503/error), `sessions` (list/error), `session_messages` (success + existing 404), `chat` server error, and `chat_stream` session_id + tool_call.

## `mimir-core`

- `job_queue/`: 23 tests for `JobPriority::from_i16`, `JobRunStatus` str roundtrip + fallback, `DailySchedule` parse/`as_hhmm`/`next_after` (TZ-robust, including non-zero-padded `%H:%M` acceptance), `JobError` predicates, enum serde roundtrips, `JobContext`, and the cgroup resource-limit helpers (`resources.rs`: cgroup v2 path parsing, name sanitisation, guard apply/restore).
- `tools/output.rs`: 10 tests for `to_display_text` (error precedence, string unquoting, non-string JSON, stdout fallback, empty/placeholder) and `to_llm_text`/`output_to_llm_text` (all-parts join, empty stdout/stderr omission, `skip_serializing_if` roundtrip).
- `tools/permission.rs`: 5 tests for default, `as_str`/`from_str` roundtrip, case-insensitive parsing, serde lowercase rename.
- `tools/error.rs`: 3 tests for constructor variants, `Into<String>` acceptance, `Display` formatting.

279 lib tests (up from 211).

## `mimir-knowledge`

- `models/enums.rs`: 6 tests — `RecurrenceType::try_from<i16>` (valid/invalid), discriminant stability, nonzero invariants, serde roundtrip.
- `retrieval/types.rs`: 9 tests — `RetrievedContext::summary` counts, `RetrievedFact::same_identity` (equality, bit-pattern for `+0.0`/`-0.0`, status/inferred/temporal differences), serde roundtrip.
- `inference/rules/transitivity.rs`: 7 tests for `intersect_windows` (unbounded, max-from, min-until, overlapping, disjoint).
- `models/entity_date.rs`: 7 tests for private helpers `is_leap_year`, `days_in_month` (incl. invalid), `parse_base_datetime` (RFC3339, date-only, offset normalisation, invalid).
- `models/memory.rs`: 7 tests for `MemoryPriority::boost` ordering/exact, discriminant stability, `MemorySchema::new`/`default`/`all_facts` ordering, serde roundtrip.
- `models/source.rs`: 2 tests — `SourceType::try_from<i16>` roundtrip (valid/invalid discriminants) and `as_str` wire-contract names.
- `models/fact.rs`: 6 tests — `FactStatus` discriminant roundtrip, `Fact::status` mapping, `try_from<i16>` roundtrip, `as_str` wire-contract names, `FromStr` wire-string parsing (incl. case-insensitive), `NewFact` defaults.
- `models/audit_log.rs`: 5 tests — `ChangeType` / `ChangedBy` `try_from<i16>` roundtrips (valid/invalid discriminants), `as_str` wire-contract names (incl. `content_update`), and `ChangeType` `FromStr` wire-string parsing (incl. case-insensitive).
- `models/entity.rs`: 6 tests — discriminant stability, the `ENTITY_TYPES` const-array lock-step contract, `try_from<i16>` roundtrip, `as_str` wire-contract names, `FromStr` wire-string parsing (incl. case-insensitive), basic construction.
- `tools/`: 1 test — the `kg_*` name helpers (`fact_status_name` / `source_type_name` / `entity_type_name`) match the wire contract with the `Unknown({id})` fallback.
- `memory/` (`queries/memory/tests.rs`): 13 tests — calendar-day upcoming suffixes, temporal-boost values (zero/one/past/none), priority boost, bucket-id mapping (every seeded bucket plus General fallback for unset/unknown ids), schema/unknown-relationship rendering, connector-predicate grammar, char estimates.

204 lib tests (up from 110).

## `mimir-server`

- `error.rs`: 16 tests for every `ApiError` response helper — status codes, error codes, `Retry-After` header on `QueueFull`, and (security-relevant) verification that internal error details (context IDs, LLM upstream text, memory I/O messages, KG internal variants) are masked from clients.
- `routes/kb/helpers.rs`: 5 tests — `status_name` / `source_type_name` / `change_type_name` / `changed_by_name` wire-contract strings (incl. `content_update` and `Unknown` fallback) and `parse_status` wire-string parsing.
- `server.rs`: 4 tests — the child-process SIGHUP registration regression (`spawn_sighup_reload_handler` must register the handler synchronously before spawning its task, so a SIGHUP sent immediately after the call is caught and reloads the config instead of killing the process via the default disposition; issue #369, same pattern as the SIGTERM regression in `shutdown.rs`), the shutdown-before-first-poll race for the SIGHUP handler (issue #421), and two config-watcher tests for issue #415: `test_config_watcher_thread_exits_when_runtime_dropped_without_shutdown` (dropping a runtime that spawned the watcher without firing the shutdown watch must not hang the blocking-pool join) and `test_config_watcher_reloads_on_file_change` (a content change on the config file is debounced, forwarded, and reloaded). Both watcher tests wait for a test-only readiness signal sent after `debouncer.watch` registers the directory, so the runtime drop and the file rewrite cannot race the asynchronous watcher registration (PR #437 review).
- `shutdown.rs`: 6 tests — shutdown-source attribution strings, graceful-vs-untriggered exit messages, the `serve_with_bounded_drain` lifetime/drain bound, the already-fired-trigger race, the child-process SIGTERM registration regression, and `test_server_exits_after_stop`. The `/stop` round trip is fully isolated (issue #384): it injects a known API token and mock LLM via `start_server_with_llm`, points context/knowledge/scheduler DBs into a temp dir (no real `~/.local/share/mimir` handles), and owns the server task behind a kill-on-drop guard so a panicking assertion cannot leak a live server into parallel suites.

49 lib tests on Unix (up from 38); non-Unix platforms have 46 because the two SIGHUP regression tests in `server.rs` and the SIGTERM regression test in `shutdown.rs` are Unix-only (the two file-watcher regression tests in `server.rs` are cross-platform and run everywhere).

## `mimir-connectors`

- `email/`: 150 tests — deterministic JSON-LD extraction (`email/jsonld`, 56: block detection, array flattening, flight/event-reservation facts, and the predicate-registration pin for every extractor family, issue #412), the connector's extract/imap/kb/llm layers (`email/connector`, 47: iMIP invite extraction, cancel tombstones, IMAP sync/cursor semantics, universal funnel, bounded LLM retry, and the non-canonical-predicate drop, issue #412), the LLM retry ledger (`email/llm`, 29), config parsing (`email/config`, 15: auth-method resolution incl. the shared discriminant contract, polling/IDLE mode selection), and IMAP auth (`email/imap`, 3: XOAUTH2 SASL, secret redaction).
- `oauth/`: 40 tests — the interactive PKCE flow (`oauth/pkce`, 17: callback parsing, state-mismatch abort, HTTPS gates, timeouts), token refresh (`oauth/refresh`, 21: grant posting, loopback-host validation, expiry clamping, error surfacing), and the HTTP client (`oauth/http_client`, 2: response-size bound).
- `photos/`: 32 tests — EXIF GPS/datetime parsing, cursor classification and pruning, reverse-geocode retry bounds, the `took_photo_at` / `visited` fact overlay, and the emitted-predicate registration gate (issue #412).
- `supervisor/`: 23 tests — runner control/forget/instantiate (20: start/pause/resume, per-connector lifecycle lock, cursor + durable-state injection) and cycle semantics (3: cursor adoption only after success, deletion replay).
- `rate_limit/`: 19 tests — backoff/jitter saturation, `Retry-After` handling, and the daily-quota tracker.
- `calendar/`: 19 tests — CalDAV collection/sync-collection parsing (13: tombstones, sync tokens, 507 truncation), credential resolution (5: OAuth refresh, auth-kind mismatch), and config parsing (1: auth-kind discriminant vs serde `kind` tag).
- `geocoder/`: 12 tests — Nominatim query encoding, locality short-name fallback chain, and place-to-result mapping.
- `ical/`: 11 tests — iCalendar datetime/TZID parsing, `vevent` → fact mapping, and the emitted-predicate registration gate (issue #412).
- `connector.rs`: 7 tests — `ConnectorContext` secret-store / user-identity plumbing.
- `test_utils.rs`: 6 tests — the shared OAuth test doubles (feature `test-utils`, #290/#298): authorize-URL parsing, callback URL building, and the wiremock token-endpoint mock.
- `secrets/`: 6 tests — slug validation, `SecretBundle` / store debug redaction, and the shared auth-mismatch message helper.
- `fact.rs`: 1 test — `ConnectorFact` shared defaults and per-fact overrides.

326 lib tests. The `test-mock-oauth`-gated in-process mock OAuth server (`mock_oauth.rs`, #207) has no inline unit tests of its own — its correctness is pinned by the PKCE E2E suites in `mimir-connectors/tests/` and `mimir/tests/` (see `docs/e2e-testing.md` for the crate's integration-test split).

## `mimir` (binary)

- `connector/`: 68 tests — CLI connector flows: `parse_duration` (bare seconds, units, case/whitespace, garbage, overflow), config merging (`merge_config` nesting/dotted keys/JSON base/overrides, `parse_config_scalar` quoting/arrays/objects/malformed fallback), credential-kind detection, `title_case`, server-error rendering, connector-not-running detection, the wiremock-backed command flows (`add`, `auth`, `act`, `forget`, `pause_and_resume`, `remove`, `sync`, `resolve_connector`, OAuth PKCE config extraction and token ingest), the legacy `gmail`→`email` alias normalization (issue #400), and the interactive wizard (scripted-prompt driver covering every email preset — Gmail OAuth defaults and app-password fallback, Outlook OAuth-only (Microsoft endpoints — app passwords retired), Yahoo / Proton Bridge / iCloud app-password defaults, custom IMAP free-form and user-supplied OAuth endpoints with empty scopes rejected — plus the calendar presets — Google Calendar computed CalDAV URL, iCloud and Yahoo defaults, custom CalDAV OAuth scopes — credential-less Photos registration, unknown-backend flag-form hint, required-field errors, sync-mode/backfill mapping (issue #397), `slugify`/`parse_scopes` helpers, and the password-prompt regression pinning that wizard secrets are asked exactly once with confirmation disabled — issue #399).
- `kb/`: 19 tests for `parse_datetime` (RFC3339, date-only, ISO without zone, space separator, fractional seconds, explicit offset, invalid), `confidence_color` boundary semantics, `truncate` (short/exact/long+ellipsis/multibyte/`max=0`), heatmap rendering (section totals and bar scaling), and the KB reset flow (wrong phrase aborts, confirmed phrase wipes and reports a backup).
- `daemon_guard.rs`: 10 tests — the auto-start guard's HTTP probe, prompt, spawn, timeout, and child-env secret-stripping paths.
- `chat.rs`: 6 tests for `format_markdown_for_terminal` fence spacing (start, middle, consecutive fences, end, empty input, no fences).
- `cli_util.rs`: 1 wiremock test — `client_with_token` falls back to a tokenless client when the token is rejected as an invalid header value.

104 bin tests (up from 29).

## Running

```bash
# Whole workspace
cargo test --workspace

# A single crate's lib tests
cargo test -p mimir-core --lib
cargo test -p mimir-knowledge --lib
cargo test -p mimir-server --lib
cargo test -p mimir-connectors --lib

# Binary crate inline tests
cargo test -p mimir --bin mimir
```

`cargo clippy --workspace --all-targets --tests --benches -- -D warnings` and `cargo fmt --all -- --check` are clean on this branch.
