# Testing and Benchmarks

## What changed

A `tests-and-benchmarks` pass massively expanded Mimir's automated test and
benchmark coverage and triaged the findings into prescriptive GitHub issues
(#160–#168).

## How Mimir is tested

- **Unit tests** (`#[cfg(test)]` modules inside each crate) cover pure helpers,
  wire-type (de)serialisation, and HTTP error mapping. They run in
  milliseconds and need no network.
- **Integration tests** (`<crate>/tests/*.rs`) exercise SQLite-backed and
  wiremock-backed pathways end to end.
- **Benchmarks** (criterion) measure both hotpaths (context manager, KG
  inference, memory condensation) and non-hotpath pure helpers
  (FTS5 escaping, confidence scoring, serde roundtrips).

## Running them

```bash
cargo test --workspace                 # all unit + integration tests
cargo bench --workspace                # all benchmarks (slow)
cargo bench -p mimir-core --bench pure_helpers
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## What got better

- Inline unit tests grew from ~12 to 46 (`mimir-api-types`), ~24 to 64
  (`mimir-client`), 179 to 211 (`mimir-core` lib), ~74 to 110
  (`mimir-knowledge` lib), 50 to 65 (`mimir-server`), and 15 to 29 (`mimir` bin).
- Three new pure-helper benchmark suites cover pathways that were previously
  unbenchmarked.
- Security-relevant tests now lock in that internal error details (LLM
  upstream text, context IDs, memory I/O messages, KG internal variants) are
  masked from HTTP clients with stable error codes.

## Use cases

- **Before changing a pure helper:** run the matching `--lib` tests to catch
  regressions in edge cases (e.g. `+0.0` vs `-0.0` confidence, FTS5 phrase
  boundaries, daily-schedule DST arithmetic).
- **Before changing a wire type:** the `roundtrip_tests!` macro documents the
  exact serialisation contract (which fields are omitted when `None`).
- **Before optimising:** capture a benchmark baseline with `cargo bench` and
  compare after the change.

## Best practices

- Keep new tests deterministic and parallel-safe — never mutate process-global
  state (no `std::env::set_var`); use dependency injection or temp dirs.
- Prefer pure unit tests for edge cases; reserve integration tests for
  DB/HTTP pathways.
- Add a benchmark whenever you make a non-trivial pure helper that could
  become a hotpath.

## Follow-up issues

See GitHub issues #160–#168 for prescriptive refactoring, performance, and
security improvements identified during the pass.
