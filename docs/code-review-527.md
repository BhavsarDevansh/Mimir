# Code Review: Issue #527

## Scope

- `Cargo.toml`
- `mimir-knowledge/src/db/migrations/059_add_fact_subject_relationship_index.sql`
- `mimir-knowledge/tests/migrations_test.rs`
- `docs/benchmarks.md`
- `docs/fact-management.md`
- `docs/kg-tools.md`
- `docs/knowledge-graph-schema.md`
- `docs/wiki/facts.md`
- `docs/wiki/what-works-now.md`

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
| Public API surface | No public API changes; migration 059 is additive. | None |

## Actions Taken During Review

- Replaced the initially proposed four-column index with a two-column subject/relationship index after benchmarking showed the extra object and confidence columns caused a 4.6% dedup benchmark regression without affecting SQLite's chosen plan.
- Added an `EXPLAIN QUERY PLAN` regression test to pin the overlap scan to the composite index.
- Re-ran the three issue-named benchmarks before and after the change and documented the short-run results.
- Re-ran the workspace test suite, Clippy with warnings denied, and formatting checks after the review fixes.

The final review returned zero open findings.
