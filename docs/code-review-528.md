# Code Review: Issue #528

## Scope

- `Cargo.toml`
- `docs/benchmarks.md`
- `docs/code-review-528.md`
- `docs/nightly-optimization.md`
- `docs/wiki/nightly-optimization.md`
- `docs/wiki/what-works-now.md`
- `mimir-knowledge/src/optimization/passes.rs`
- `mimir-knowledge/tests/optimization_tests.rs`

## Review Findings

| Dimension | Finding | Severity |
|---|---|---|
| Code quality | None | None |
| Performance | None | None |
| Security | None | None |
| Doc comments | None | None |
| DRY compliance | None | None |
| Modern design patterns | None | None |
| Guideline compliance | None | None |
| VISION compliance | None | None |
| Type consistency | None | None |
| Public API surface | No public API changes. | None |

## Follow-Up Review

| Dimension | Finding | Severity | Action |
|---|---|---|---|
| Correctness | The deterministic-dedup candidate query used closed interval comparisons, while fact insertion uses half-open intervals and excludes empty ranges. | Medium | Replaced sentinel `COALESCE` comparisons with explicit half-open SQL predicates and added an empty-interval regression test. |
| Correctness | `COALESCE` object comparisons treated SQL `NULL` and an empty literal as the same object. | Medium | Switched to null-aware `IS` comparisons and added a regression test. |
| Code quality | The candidate rows were represented by an opaque four-field tuple. | Low | Replaced the tuple with a local `MergeCandidate` struct. |
| DRY compliance | Tests repeated direct fact-and-source seeding SQL. | Low | Extracted an `insert_unmanaged_fact` test helper. |
| Correctness | The review documentation claimed pass-level rollback coverage, but no test exercised a failure after the first merge. | Low | Added a fault-injection regression test that verifies the first merge, confidence boost, provenance transfer, and dependency writes roll back together. |
| Guideline compliance | The workspace version was `0.153.10`, while the release target and version-linked documentation used `0.153.9`. | Low | Aligned the workspace version to `0.153.9`. |

## Actions Taken During Review

- Replaced per-pair transactions with one pass-level transaction and moved both confidence values into the candidate query.
- Tracked the current keeper confidence in Rust so successive duplicate merges do not restart from stale snapshots.
- Added regression coverage for multiple duplicate merges, provenance transfer, supersession dependencies, pass-level rollback, and empty intervals.
- Documented the v0.153.9 benchmark delta and updated technical and user-facing optimization documentation.
- Filed #548 for the pre-existing semantic-dedup object comparison issue outside this change set.
- Re-ran formatting, the targeted optimization suite, the workspace test suite, the full workspace Clippy check with warnings denied, and the targeted dedup benchmark.
- Re-reviewed the release-version alignment and re-ran formatting, the workspace test suite, and the full workspace Clippy check with warnings denied.

The final review returned zero open findings.
