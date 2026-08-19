# Testing and Benchmarks

## What changed

A `tests-and-benchmarks` pass massively expanded Mimir's automated test and benchmark coverage and triaged the findings into prescriptive GitHub issues (#160–#168).

## How Mimir is tested

- **Unit tests** (`#[cfg(test)]` modules inside each crate) cover pure helpers, wire-type (de)serialisation, and HTTP error mapping. They run in milliseconds and need no network.
- **Integration tests** (`<crate>/tests/*.rs`) exercise SQLite-backed and wiremock-backed pathways end to end.
- **Daemon E2E tests** (`mimir/tests/*.rs`) boot the real daemon in-process (mock LLM, isolated temp HOME/XDG) and drive the real `mimir` CLI binary against it — the full connector lifecycle plus the mock-connector sync → `normalize_and_insert` → KB-query round trip with provenance, reliability-score confidence, and corroboration assertions (issue #206), and the OAuth PKCE login against an in-process mock OAuth server via a `$BROWSER` fake browser (issue #207).
- **Connector E2E tests** (`mimir-connectors/tests/*.rs`) cover the PKCE flow against the mock OAuth server (HTTPS authorize + HTTP token endpoints, PKCE S256 validation, one-time codes), the rate-limit/backoff primitives over real HTTP (429/503 with `Retry-After`, daily-quota exhaustion), and the supervisor edge cases (startup restore, shutdown cursor persistence, circuit breaker, panic recovery) — issue #207.
- **Shared test doubles** (`mimir-connectors::test_utils`, feature `test-utils`, issues #290, #298) own the fake-browser opener (`self_callback_opener`), authorize-URL parsing (`parse_authorize_url` / `callback_url`), and the wiremock token-endpoint mock (`mount_token_endpoint`) that the PKCE flow unit tests and the CLI connector tests both use, so the two suites can never drift apart.
- **Benchmarks** (criterion) measure both hotpaths (context manager, KG inference, memory condensation) and non-hotpath pure helpers (FTS5 escaping, confidence scoring, serde roundtrips).

## Running them

```bash
cargo test --workspace                 # all unit + integration tests
cargo bench --workspace                # all benchmarks (slow)
cargo bench -p mimir-core --bench pure_helpers
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## What got better

- Inline unit tests grew from ~12 to 63 (`mimir-api-types`), ~24 to 74 (`mimir-client`), 211 to 279 (`mimir-core` lib), 110 to 204 (`mimir-knowledge` lib), 38 to 44 (`mimir-server`), and 15 to 69 (`mimir` bin); `mimir-connectors` currently carries 321 lib tests across the email/oauth/photos/supervisor/rate-limit/calendar/geocoder/ical/secrets modules (see `docs/unit-tests.md`).
- Three new pure-helper benchmark suites cover pathways that were previously unbenchmarked.
- Security-relevant tests now lock in that internal error details (LLM upstream text, context IDs, memory I/O messages, KG internal variants) are masked from HTTP clients with stable error codes.

## Use cases

- **Before changing a pure helper:** run the matching `--lib` tests to catch regressions in edge cases (e.g. `+0.0` vs `-0.0` confidence, FTS5 phrase boundaries, daily-schedule DST arithmetic).
- **Before changing a wire type:** the `roundtrip_tests!` macro documents the exact serialisation contract (which fields are omitted when `None`).
- **Before optimising:** capture a benchmark baseline with `cargo bench` and compare after the change.

## Best practices

- Keep new tests deterministic and parallel-safe — never mutate process-global state (no `std::env::set_var`); use dependency injection or temp dirs.
- Prefer pure unit tests for edge cases; reserve integration tests for DB/HTTP pathways.
- Add a benchmark whenever you make a non-trivial pure helper that could become a hotpath.

## Follow-up issues

See GitHub issues #160–#168 for prescriptive refactoring, performance, and security improvements identified during the pass.

## Review-fix refinements (v0.54.5)

Following review of the test/benchmark pass, a few test-quality refinements landed (no user-facing behaviour changed):

- Sparse-field serde tests now check actual JSON keys rather than doing a substring search, so they cannot be fooled by field names appearing inside values.
- KB client tests now assert the query parameters each method sends, not just the route path, so query-encoding regressions are caught.
- The daily-schedule benchmark uses a fixed reference time so its baseline is reproducible across runs.
- A new test documents that `DailySchedule::parse` accepts non-zero-padded times like `"2:30"` (chrono's `%H:%M` parser is padding-agnostic).
