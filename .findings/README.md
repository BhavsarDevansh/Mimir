# Triage findings — tests-and-benchmarks branch

Prescriptive issues to be filed on GitHub. Each entry: crate, finding, severity, labels, suggested fix.

## F1 — api-types: inconsistent `skip_serializing_if` on KG wire types
Several KG response/request structs serialize `Option` fields as `null` instead of omitting them, unlike ChatRequest/StatusResponse which use `#[serde(skip_serializing_if = "Option::is_none")]`.
Affected: `AuditRow.{entity_name, predicate_name, changed_by}`, `CategoryResponse.{description, parent_id, memory_weight}`, `CategoryDetailResponse.{description, parent_id, memory_weight}`, `TrashRow.{subject, predicate, object}`, `OptimizationStatusResponse.{schedule, next_run_at, last_run}`, `OptimizationRunNowResponse.{finished_at, error}`, `OptimizationRunSummary.{finished_at, error}`, `PendingFactRow.object` is correctly skipped; `SourceRow`/`FactRow` correct.
Fix: add `#[serde(skip_serializing_if = "Option::is_none")]` to the listed fields for payload-size and consistency.
Severity: low (performance/api hygiene). Labels: api-types? none exists → use `knowledge-graph`, `performance`, `core-agent`.
