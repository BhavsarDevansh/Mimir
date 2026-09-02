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

## Actions Taken During Review

- Replaced per-pair transactions with one pass-level transaction and moved both confidence values into the candidate query.
- Tracked the current keeper confidence in Rust so successive duplicate merges do not restart from stale snapshots.
- Added regression coverage for multiple duplicate merges, provenance transfer, supersession dependencies, and empty intervals.
- Documented the v0.153.9 benchmark delta and updated technical and user-facing optimization documentation.
- Re-ran formatting, the targeted optimization suite, the workspace test suite, the full workspace Clippy check with warnings denied, and the targeted dedup benchmark.

The final review returned zero open findings.
