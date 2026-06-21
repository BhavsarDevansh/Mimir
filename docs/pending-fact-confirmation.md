# Pending Sensitive-Fact Confirmation

> **Scope:** `mimir-knowledge`, `mimir-server`, `mimir-client`, `mimir`,
> `mimir-api-types`
> **Issue:** #141
> **Vision:** `VISION/02-Knowledge-Graph/Learning-Modes.md` (Confirmation Flow)

---

## Overview

Sensitive facts (allergies, health, financial details, etc.) detected by the
extraction pipeline are stored with `pending_confirmation = TRUE` and
`fact_status_id = Disputed`. They must be explicitly confirmed or rejected by
the user before they influence the graph. Previously, `confirm_fact` /
`reject_fact` existed only as internal Rust APIs and pending facts sat in limbo
forever. This change exposes the full lifecycle over HTTP, the CLI, and an
automated cleanup job.

## Lifecycle

1. **Detection** — the `remember` tool flags `is_sensitive: true`; the
   extractor inserts the fact with `Disputed` status and
   `pending_confirmation = TRUE` (`mimir-knowledge/src/extract.rs`,
   `insert_sensitive_fact`). The fact id is added to the in-memory
   `pending_confirmations` set.
2. **Confirm** — `KnowledgeGraph::confirm_fact(id)` flips status to `Active`,
   sets confidence to `1.0`, clears `pending_confirmation`, writes a
   `StatusChange` audit entry, and runs the inference cascade.
3. **Reject** — `KnowledgeGraph::reject_fact(id, reason)` writes a `Rejected`
   audit entry (with optional reason), hard-deletes the fact, and removes it
   from the cache. Sources cascade; audit rows persist.
4. **Auto-cleanup** — the `knowledge.pending_cleanup` daily job hard-deletes
   any fact still awaiting confirmation past `retention_days`.

## Data access (`mimir-knowledge`)

| Method | Purpose |
|--------|---------|
| `KnowledgeGraph::list_pending_facts()` | `SELECT` pending facts joined to resolve subject/predicate/object names |
| `KnowledgeGraph::delete_stale_pending(retention_days)` | Hard-delete stale pending facts using `self.now()` (clock-injectable for tests) |
| `queries::fact::PendingFactRow` | Row model with resolved names |
| `extract::confirm_fact` / `extract::reject_fact` | Existing transactional confirm/reject (reject now takes `Option<&str>` reason) |

`delete_stale_pending` uses the configured `Clock` (`self.now()`) so tests can
fast-forward time via `clock::MockClock` rather than mutating process
environment (banned by the safety policy).

## HTTP API (`mimir-server`)

| Method | Route | Handler | Response |
|--------|-------|---------|----------|
| `GET` | `/kb/pending` | `kb_pending_handler` | `PendingListResponse` |
| `POST` | `/kb/facts/{id}/confirm` | `kb_confirm_fact_handler` | `ConfirmFactResponse` (`FactRow`, status `Active`) |
| `POST` | `/kb/facts/{id}/reject` | `kb_reject_fact_handler` | `204 No Content` |

Confirming a non-pending fact returns `400 Bad Request` (`Validation` error).
Reject accepts an optional JSON body `{"reason": "..."}`; an empty body is
valid. The shared `fact_row_from` helper resolves names for both confirm and
the existing edit endpoint (DRY).

## API types (`mimir-api-types`)

- `PendingFactRow` — `fact_id`, `subject`, `predicate`, `object`, `created_at`
- `PendingListResponse` — `{ total, facts: Vec<PendingFactRow> }`
- `ConfirmFactResponse` — `{ fact: FactRow }` (consistent with `FactEditResponse`)
- `RejectFactRequest` — `{ reason: Option<String> }` (all fields optional)

## CLI (`mimir`)

```bash
mimir kb pending [--json]
mimir kb confirm <fact-id> [--json]
mimir kb reject <fact-id> [--reason "..."]
```

## Client (`mimir-client`)

`MimirClient::kb_pending`, `kb_confirm`, `kb_reject(reason: Option<&str>)`.

## Background job (`mimir-server`)

Registered in `AppState::from_config_with_llm` alongside `knowledge.optimization`
and `memory.condensation`. A plain `Job::new` with a daily `DailySchedule`
(lightweight single `DELETE`; not added to the in-memory `DaemonJob` idle-gating
enum, which is reserved for LLM-heavy jobs). Uses `kg.delete_stale_pending()`
so the cutoff honours the injectable clock.

### Configuration

```toml
[knowledge.pending_cleanup]
retention_days = 7      # u16; facts pending longer than this are deleted
schedule_time = "03:30" # local HH:MM, 24h, daily
```

Defaults: 7 days, 03:30. Mirrors the `[knowledge.optimization]` block.

## Rationale

- **Logic in Rust** — confirm/reject/cleanup are deterministic DB operations;
  no LLM involvement.
- **Clock injection** — `MockClock` makes the 7-day retention testable without
  `set_var` (unsafe in edition 2024).
- **No new dependencies** — reuses axum, sqlx, clap, chrono patterns already
  in the workspace.
- **Internal API break** — `reject_fact` gained a `reason` parameter;
  acceptable per the project's breaking-changes policy.
