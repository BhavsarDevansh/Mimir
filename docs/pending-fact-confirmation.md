# Pending Sensitive-Fact Confirmation

> **Scope:** `mimir-knowledge`, `mimir-server`, `mimir-client`, `mimir`, `mimir-api-types`
>
> **Issue:** #141
>
> **Vision:** `VISION/02-Knowledge-Graph/Learning-Modes.md` (Confirmation Flow)

---

## Overview

Sensitive facts (allergies, health, financial details, etc.) detected by the extraction pipeline are stored with `pending_confirmation = TRUE` and `fact_status_id = Disputed`. They must be explicitly confirmed or rejected by the user before they influence the graph. Previously, `confirm_fact` / `reject_fact` existed only as internal Rust APIs and pending facts sat in limbo forever. This change exposes the full lifecycle over HTTP, the CLI, and an automated cleanup job.

## Lifecycle

1. **Detection** — the extraction pipeline (driven by the `remember.chat` hook since #386) flags `is_sensitive: true`; the Rust sensitivity gate (`mimir-knowledge/src/sensitivity.rs`, #142) validates this flag against the fact's catalogue categories (`SENSITIVE_CATEGORIES` constant) and object-text keywords (`SENSITIVE_KEYWORDS` constant). Only facts that pass both the LLM flag and the Rust check are inserted with `Disputed` status and `pending_confirmation = TRUE` (`mimir-knowledge/src/extract/`, `insert_sensitive_fact`). The fact id is added to the in-memory `pending_confirmations` set.
2. **Confirm** — `KnowledgeGraph::confirm_fact(id)` flips status to `Active`, sets confidence to `1.0`, clears `pending_confirmation`, writes a `StatusChange` audit entry, and runs the inference cascade. It also rebuilds any overlays that were deferred at extraction time: the events-subsystem overlay from `pending_event_meta` (migration 041) and the entity-locations overlay from `pending_location_meta` (migration 048, issue #226 — a confirmed sensitive "where" fact produces the same `entity_locations` row as a non-sensitive one, geocoded with the confirmed fact's temporal bounds and `source_fact_id`). The location shape is persisted atomically with the pending fact at extraction time, and the meta row is consumed only after the overlay write succeeds — a failed write retains it for retry instead of losing the only location payload. Rejecting hard-deletes the fact, and both meta tables cascade-delete with it, so no orphan overlay rows can be left behind.
3. **Reject** — `KnowledgeGraph::reject_fact(id, reason)` writes a `Rejected` audit entry (with optional reason), clears any `fact_dependencies` rows referencing the fact (required by the `ON DELETE RESTRICT` FK from migration 017), hard-deletes the fact, and removes it from the cache. Sources cascade; audit rows persist.
4. **Auto-cleanup** — the `knowledge.pending_cleanup` daily job hard-deletes any fact still awaiting confirmation past `retention_days`. The nightly optimization runner's `pending_confirmation_cleanup` pass performs the same expiry using the same configured `retention_days` (see Configuration).

## Data access (`mimir-knowledge`)

| Method | Purpose |
|--------|---------|
| `KnowledgeGraph::list_pending_facts()` | `SELECT` pending facts joined to resolve subject/predicate/object names |
| `KnowledgeGraph::delete_stale_pending(retention_days)` | Hard-delete stale pending facts using `self.now()` (clock-injectable for tests) |
| `queries::fact::PendingFactRow` | Row model with resolved names |
| `extract::confirm_fact` / `extract::reject_fact` | Existing transactional confirm/reject (reject now takes `Option<&str>` reason) |

`delete_stale_pending` uses the configured `Clock` (`self.now()`) so tests can fast-forward time via `clock::MockClock` rather than mutating process environment (banned by the safety policy). It re-checks the stale predicate inside each per-fact transaction and only counts committed deletes, so a fact confirmed or rejected between the id scan and the delete is skipped rather than incorrectly hard-deleted or audited.

## HTTP API (`mimir-server`)

| Method | Route | Handler | Response |
|--------|-------|---------|----------|
| `GET` | `/kb/pending` | `kb_pending_handler` | `PendingListResponse` |
| `POST` | `/kb/facts/{id}/confirm` | `kb_confirm_fact_handler` | `ConfirmFactResponse` (`FactRow`, status `Active`) |
| `POST` | `/kb/facts/{id}/reject` | `kb_reject_fact_handler` | `204 No Content` |

Confirming a non-pending fact returns `400 Bad Request` (`Validation` error). Reject accepts an optional JSON body `{"reason": "..."}`; an empty body is valid. The shared `fact_row_from` helper resolves names for both confirm and the existing edit endpoint (DRY).

All three routes are wrapped in the `require_loopback` middleware (the same guard used for `/kb/optimization/run-now`, `/memory/refresh`, and `/stop`), so only loopback peers can list or mutate pending sensitive facts. There is no browser frontend for these routes (the CLI / `mimir-client` is the only client and issues non-browser requests), so no CSRF/`Origin` validation is applied; that hardening belongs to a workspace-wide pass over all mutation routes, not a one-off on the sensitive-fact surface.

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

Registered in `AppState::from_config_with_llm` alongside `knowledge.optimization` and `memory.condensation`. A plain `Job::new` with a daily `DailySchedule` (lightweight single `DELETE`; not added to the in-memory `DaemonJob` idle-gating enum, which is reserved for LLM-heavy jobs). Uses `kg.delete_stale_pending()` so the cutoff honours the injectable clock.

### Configuration

```toml
[knowledge.pending_cleanup]
retention_days = 7      # u16; facts pending longer than this are deleted
schedule_time = "03:30" # local HH:MM, 24h, daily
```

Defaults: 7 days, 03:30. Mirrors the `[knowledge.optimization]` block. The nightly optimization runner reads `retention_days` into `OptimizationConfig.pending_cleanup_retention_days` so its `pending_confirmation_cleanup` pass and the scheduled `knowledge.pending_cleanup` job share one configured expiry window.

## Rationale

- **Logic in Rust** — confirm/reject/cleanup are deterministic DB operations; no LLM involvement.
- **Clock injection** — `MockClock` makes the 7-day retention testable without `set_var` (unsafe in edition 2024).
- **No new dependencies** — reuses axum, sqlx, clap, chrono patterns already in the workspace.
- **Internal API break** — `reject_fact` gained a `reason` parameter; acceptable per the project's breaking-changes policy.
