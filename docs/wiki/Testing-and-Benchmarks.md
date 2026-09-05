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

The deterministic memory benchmark harness (issue #568) adds quality and performance budgets for the current knowledge graph and memory pipeline. It is off by default and omitted from default production builds. Run `cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark` to emit JSON or use `--save-baseline`/`--baseline` for local comparisons. See `docs/memory-benchmark.md`.

## Running them

```bash
cargo test --workspace                 # all unit + integration tests
cargo bench --workspace                # all benchmarks (slow)
cargo bench -p mimir-core --bench pure_helpers
scripts/perf-baseline.sh               # suite wall-time + slowest tests (needs cargo-nextest)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
scripts/tests/rustdoc_test.sh
```

## What got better

- Inline unit tests grew from ~12 to 63 (`mimir-api-types`), ~24 to 74 (`mimir-client`), 211 to 279 (`mimir-core` lib), 110 to 204 (`mimir-knowledge` lib), 38 to 49 (`mimir-server`; 46 on non-Unix platforms — the two SIGHUP regression tests in `server.rs` and the SIGTERM regression test in `shutdown.rs` are Unix-only, while the two file-watcher regression tests are cross-platform), and 29 to 84 (`mimir` bin); `mimir-connectors` currently carries 326 lib tests across the email/oauth/photos/supervisor/rate-limit/calendar/geocoder/ical/secrets modules (see `docs/unit-tests.md`). The two file-watcher regression tests wait for successful watcher registration (a test-only readiness signal after `debouncer.watch` succeeds) before dropping the runtime or rewriting the config, so they deterministically exercise the registered watch instead of racing the asynchronous `spawn_blocking` start (PR #437 review).
- Three new pure-helper benchmark suites cover pathways that were previously unbenchmarked.
- Security-relevant tests now lock in that internal error details (LLM upstream text, context IDs, memory I/O messages, KG internal variants) are masked from HTTP clients with stable error codes.
- The calendar knowledge-graph integration tests are deterministic under parallel load: they wait for the events-subsystem overlay (`get_event_by_fact`) instead of the bare fact list, and drive the tombstone cycle via `trigger_sync_by_slug` — closing the initial-cycle and tombstone-cycle races that previously made `calendar_kb_tests` flaky in full-suite runs (issues #320, #367).
- The workspace suite is isolated from the developer's real install (issue #384): the daemon-down CLI tests probe the never-bindable loopback port 0 with temp HOME/XDG instead of the default base URL, every daemon-spawning test runs against temp DBs with an injected API token, and the in-process `TestDaemon` / server-task fixtures kill their server on drop so a panicking test cannot leak a daemon that locks the real `knowledge.db`/`jobs.db` or holds a port.
- Retry tests can inject short schedules instead of sleeping through production backoff: the LLM client accepts a positive-attempt `RetryConfig`, and connector HTTP tests bound `Retry-After` to the injected strategy cap (issue #531).
- Core hook and scheduler tests now synchronise on readiness signals, pending-state polling, or Tokio mock time instead of multi-hundred-millisecond wall-clock sleeps (issue #533), making the timing-window tests deterministic and faster.

## Use cases

- **Before changing a pure helper:** run the matching `--lib` tests to catch regressions in edge cases (e.g. `+0.0` vs `-0.0` confidence, FTS5 phrase boundaries, daily-schedule DST arithmetic).
- **Before changing a wire type:** the `roundtrip_tests!` macro documents the exact serialisation contract (which fields are omitted when `None`).
- **Before optimising:** capture a benchmark baseline with `cargo bench` and compare after the change.

## Best practices

- Keep new tests deterministic and parallel-safe — never mutate process-global state (no `std::env::set_var`); use dependency injection or temp dirs.
- Prefer pure unit tests for edge cases; reserve integration tests for DB/HTTP pathways.
- Add a benchmark whenever you make a non-trivial pure helper that could become a hotpath.
- Keep public intra-doc labels free of redundant explicit targets; the guard fails redundant-explicit-links warnings before merge.

## Follow-up issues

See GitHub issues #160–#168 for prescriptive refactoring, performance, and security improvements identified during the pass.

## Performance baselines (v0.153.0)

The performance investigation (2026-08-26) added criterion suites for the write/setup paths the tests pay repeatedly (`mimir-knowledge kg_write_benchmarks`, `mimir-core db_init` + `mock_llm`, `mimir-server state_build`) and a `scripts/perf-baseline.sh` script that times the whole suite with cargo-nextest. Baseline on 2026-08-26: 2315 tests in 189.3 s wall (755.9 s summed durations). Every finding from the audit is tracked as its own GitHub issue (#523–#537), each naming the benchmark to watch; see `docs/benchmarks.md` for the baseline table and reproduction commands. When fixing one of these issues, capture the baseline, apply the fix, and report the delta in the PR.

## Review-fix refinements (v0.54.5)

Following review of the test/benchmark pass, a few test-quality refinements landed (no user-facing behaviour changed):

- Sparse-field serde tests now check actual JSON keys rather than doing a substring search, so they cannot be fooled by field names appearing inside values.
- KB client tests now assert the query parameters each method sends, not just the route path, so query-encoding regressions are caught.
- The daily-schedule benchmark uses a fixed reference time so its baseline is reproducible across runs.
- A new test documents that `DailySchedule::parse` accepts non-zero-padded times like `"2:30"` (chrono's `%H:%M` parser is padding-agnostic).
