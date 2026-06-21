# Triage findings — tests-and-benchmarks branch

Prescriptive issues to be filed on GitHub. Each entry: crate, finding, severity, labels, suggested fix.

## F1 — api-types: inconsistent `skip_serializing_if` on KG wire types
Several KG response/request structs serialize `Option` fields as `null` instead of omitting them, unlike ChatRequest/StatusResponse which use `#[serde(skip_serializing_if = "Option::is_none")]`.
Affected: `AuditRow.{entity_name, predicate_name, changed_by}`, `CategoryResponse.{description, parent_id, memory_weight}`, `CategoryDetailResponse.{description, parent_id, memory_weight}`, `TrashRow.{subject, predicate, object}`, `OptimizationStatusResponse.{schedule, next_run_at, last_run}`, `OptimizationRunNowResponse.{finished_at, error}`, `OptimizationRunSummary.{finished_at, error}`, `PendingFactRow.object` is correctly skipped; `SourceRow`/`FactRow` correct.
Fix: add `#[serde(skip_serializing_if = "Option::is_none")]` to the listed fields for payload-size and consistency.
Severity: low (performance/api hygiene). Labels: api-types? none exists → use `knowledge-graph`, `performance`, `core-agent`.

## F2 — core: incomplete doc comments on JobError predicate helpers
`JobError::is_not_registered` and `is_already_running` doc comments read "Returns " with the word "true" missing.
File: mimir-core/src/job_queue.rs (lines ~224, ~229).
Fix: complete to "Returns `true` if ...".
Severity: low (doc quality). Labels: `core-agent`, `documentation`.

## F3 — core: DailySchedule::parse accepts non-zero-padded hours
`DailySchedule::parse("2:30")` succeeds even though the documented format is `HH:MM`. chrono's `%H` parser is lenient about width. This is inconsistent with the documented contract; callers expecting strict `HH:MM` may be surprised.
File: mimir-core/src/job_queue.rs (`DailySchedule::parse`).
Fix options: (a) document that single-digit hours are accepted, or (b) validate width strictly before parsing.
Severity: low. Labels: `core-agent`, `testing`.

## F4 — core: fts5 escape preserves leading/trailing whitespace inside quoted phrase
`escape_fts5("  hello  ")` returns `"\"  hello  \""` (with leading/trailing spaces inside the phrase). The trimmed check only guards the all-whitespace case, but partial-whitespace inputs keep surrounding spaces inside the quote, producing a phrase that matches with spaces. Minor: consider trimming the escaped value before quoting, or quoting `trimmed`.
File: mimir-core/src/fts5.rs.
Severity: low. Labels: `knowledge-graph`, `performance` (FTS match quality).
