# Unit Tests

This document covers the inline (`#[cfg(test)]`) unit-test coverage across the Mimir workspace. Integration tests live under each crate's `tests/` directory and are documented in `docs/integration-tests.md`.

## Scope of the `tests-and-benchmarks` pass

A workspace-wide pass expanded inline unit-test coverage for pure helpers, wire types, and response mapping that were previously only exercised indirectly (or not at all). All additions are deterministic and parallel-safe; none mutate process-global state (no `set_var`/`remove_var`).

## `mimir-api-types` (`src/lib.rs`)

63 tests (up from 12). Added a `roundtrip_tests!` macro that asserts both the populated and sparse (all-`None`) forms round-trip, and that `skip_serializing_if` fields are omitted from the sparse JSON. The sparse-field check parses the serialised JSON into a `serde_json::Map` and asserts key absence with `contains_key` (not a substring search, which could match value text). Covers every KG wire type: `FactQueryParams`, `FactRow`, `FactQueryResponse`, `SourceRow`, `DependencyRow`, `AuditRow`, `FactDetailResponse`, `FactEditRequest`, `FactEditResponse`, `BrowseRequest`, `BrowseEdge`/`BrowseResponse`, `CategoryResponse`/`CategoryDetailResponse`, `ProfileRequest`/`ProfileResponse`, `AuditQueryRequest`/`AuditQueryResponse`, `ForgetRequest`/`ForgetResponse`, `RestoreRequest`/`RestoreResponse`, `TrashRow`/`TrashListResponse`, `OptimizationRunSummary`/`OptimizationStatusResponse`/`OptimizationRunNowResponse`, `PendingFactRow`/`PendingListResponse`, `ConfirmFactResponse`, `RejectFactRequest`, plus `StreamItem` variant equality.

## `mimir-client` (`src/lib.rs`)

74 tests (up from ~24). Added pure unit tests for the SSE parser primitives (`find_double_newline`, `parse_sse_event` — text/usage/tool_call/session_id/ error/empty/multiline/invalid-UTF-8) and wiremock-backed integration tests for every previously-uncovered `MimirClient` method: `kb_optimization_status`, `kb_optimization_run_now`, `kb_query/show/edit/browse/profile/audit/forget/restore/trash/trash_empty/pending/confirm/reject`, `stop` (success/503/error), `sessions` (list/error), `session_messages` (success + existing 404), `chat` server error, and `chat_stream` session_id + tool_call.

## `mimir-core`

- `job_queue/`: 15 tests for `JobPriority::from_i16`, `JobRunStatus` str roundtrip + fallback, `DailySchedule` parse/`as_hhmm`/`next_after` (TZ-robust, including non-zero-padded `%H:%M` acceptance), `JobError` predicates, enum serde roundtrips, `JobContext`.
- `tools/output.rs`: 10 tests for `to_display_text` (error precedence, string unquoting, non-string JSON, stdout fallback, empty/placeholder) and `to_llm_text`/`output_to_llm_text` (all-parts join, empty stdout/stderr omission, `skip_serializing_if` roundtrip).
- `tools/permission.rs`: 5 tests for default, `as_str`/`from_str` roundtrip, case-insensitive parsing, serde lowercase rename.
- `tools/error.rs`: 3 tests for constructor variants, `Into<String>` acceptance, `Display` formatting.

279 lib tests (up from 179).

## `mimir-knowledge`

- `models/enums.rs`: 6 tests — `RecurrenceType::try_from<i16>` (valid/invalid), discriminant stability, nonzero invariants, serde roundtrip.
- `retrieval/types.rs`: 9 tests — `RetrievedContext::summary` counts, `RetrievedFact::same_identity` (equality, bit-pattern for `+0.0`/`-0.0`, status/inferred/temporal differences), serde roundtrip.
- `inference/rules/transitivity.rs`: 7 tests for `intersect_windows` (unbounded, max-from, min-until, overlapping, disjoint).
- `models/entity_date.rs`: 7 tests for private helpers `is_leap_year`, `days_in_month` (incl. invalid), `parse_base_datetime` (RFC3339, date-only, offset normalisation, invalid).
- `models/memory.rs`: 7 tests for `MemoryPriority::boost` ordering/exact, discriminant stability, `MemorySchema::new`/`default`/`all_facts` ordering, serde roundtrip.
- `models/source.rs`: 2 tests — `SourceType::try_from<i16>` roundtrip (valid/invalid discriminants) and `as_str` wire-contract names.
- `models/fact.rs`: 6 tests — `FactStatus` discriminant roundtrip, `Fact::status` mapping, `try_from<i16>` roundtrip, `as_str` wire-contract names, `FromStr` wire-string parsing (incl. case-insensitive), `NewFact` defaults.

195 lib tests (up from ~74).

## `mimir-server`

- `error.rs`: 15 tests for every `ApiError` response helper — status codes, error codes, `Retry-After` header on `QueueFull`, and (security-relevant) verification that internal error details (context IDs, LLM upstream text, memory I/O messages, KG internal variants) are masked from clients.
- `routes/kb/helpers.rs`: 3 tests — `status_name` / `source_type_name` wire-contract strings (incl. `Unknown` fallback) and `parse_status` wire-string parsing.

41 lib tests (up from 50).

## `mimir` (binary)

- `kb/`: 16 tests for `parse_datetime` (RFC3339, date-only, ISO without zone, space separator, fractional seconds, invalid), `confidence_color` boundary semantics, and `truncate` (short/exact/long+ellipsis/multibyte/`max=0`).

29 bin tests (up from 15).

## Running

```bash
# Whole workspace
cargo test --workspace

# A single crate's lib tests
cargo test -p mimir-core --lib
cargo test -p mimir-knowledge --lib
cargo test -p mimir-server --lib

# Binary crate inline tests
cargo test -p mimir --bin mimir
```

`cargo clippy --workspace --all-targets --tests --benches -- -D warnings` and `cargo fmt --all -- --check` are clean on this branch.
