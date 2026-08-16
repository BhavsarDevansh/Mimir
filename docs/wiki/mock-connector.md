# Mock Connector

> **Phase:** 3 — Connectors
>
> **Status:** Done (issue #190 / F13). Always compiled.

## What it is

The mock connector is Mimir's built-in test harness for the connector framework. It is a fake connector that emits pre-written ("canned") facts on a schedule you configure, so the framework and its supervisor can be tested end-to-end without connecting to any real service (no Gmail, no calendar, no photo library).

It is **always compiled** — it ships in every build, including `--no-default-features`, so the connector framework is always exercisable.

## Why it exists

Real connectors talk to external services. Testing the framework (startup, restarts, backoff, the circuit breaker, auth expiry, manual sync triggers, cursor persistence) should not depend on those services being reachable. The mock stands in: you describe what it should do in JSON, and it behaves like a real connector for the purpose of the test.

It is also the vehicle for the end-to-end "sync → extract → insert → query" test that proves the connector ingestion pipeline works before any real backend exists.

## How it works

- You write a connector row whose `backend` is the mock and whose `config_json` describes the behaviour (mode, cadence, canned facts, health, failure injection).
- The supervisor starts it like any connector: it runs `health` → `sync` → `extract` → inserts the canned facts through the same fact pipeline as a conversation (`normalize_and_insert`).
- It can also report **deletions** (via a `deletions` list in its config): the supervisor then trashes the matching knowledge-graph facts, so the full server-side-deletion path can be tested without a real service.
- The canned facts land in the knowledge graph with connector provenance, so they get the same confidence scoring, corroboration, and sensitivity gating as facts you tell Mimir directly.

### Two modes

- **Polling** — the supervisor waits `interval_ms + jitter_ms` between syncs.
- **Push** — the mock sleeps `interval_ms` inside `sync()` to self-pace, then emits. The supervisor cancels it on shutdown. Manual triggers are not supported for push connectors (the framework rejects them).

## Use cases

- **Framework tests** — drive the supervisor's lifecycle (backoff, circuit breaker, auth-expiry pause, cursor persistence, trigger preemption) with deterministic, configurable behaviour.
- **End-to-end ingestion test** — prove canned facts flow all the way into the knowledge graph with correct provenance.
- **Regression harness** — exercise both polling and push task loops.

## Best practices

- Always set a `cursor` when you want to assert sync progress was persisted.
- Use `fail_first` / `panic_first` / `always_fail` to exercise failure and recovery paths; keep these at `0` for happy-path tests.
- Use `health: "auth_expired"` to test the auth-expiry pause path.
- Give each canned fact a `raw_reference` (it is required for connector provenance); the mock auto-generates one (`mock-<slug>-<index>`) if you omit it.
- To test deletion propagation, use two cycles: ingest the fact first (e.g. `batch_size: 1` so the first cycle delivers only the fact), then let a later cycle re-stage the same `raw_reference` in `deletions` so the tombstone removes it. Deletions are processed before the same cycle's insertions, so a fact and its tombstone staged in one cycle would leave the fresh insertion in place.
- The mock is a test harness — do not register it as a real connector in production config.

## Validation guarantees

The mock validates its `config_json` up front so misconfigurations fail loudly instead of silently misbehaving:

- `__ctype` (when the supervisor injects it) must be a valid integer `ConnectorType`. An invalid value is rejected with a config error rather than silently defaulting to Gmail.
- `batch_size` must be greater than zero. A zero would let `sync()` succeed forever while fetching nothing, so it is rejected.
- The `facts` schema declares the required fields (`subject`, `relationship_type`, `object`) and the typed enums for entity/recurrence types, so a malformed fact is caught at config time.
- The schema's entity/recurrence enum lists and defaults are generated from the real enum variant arrays (`ENTITY_TYPES` / `RECURRENCE_TYPES`), not re-typed by hand — so the schema stays in sync with the enums automatically.

The sync-options recorder is cancellation-safe: it tracks each `sync()` call for its entire lifetime (including injected failures, panics, and supervisor shutdown cancellation), so the in-flight counter never leaks.

## See also

- [Connectors](connectors.md) — the connector framework overview.
- `docs/mock-connector.md` — the technical reference (config schema, public API).
- `VISION/09-Roadmap/Phase-3-Plan.md` — the full Phase 3 design.
