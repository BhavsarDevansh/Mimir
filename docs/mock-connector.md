# Mock Connector (Phase 3 F13 / #190)

> **Status:** Done. Gated by the off-by-default `test-mock-connector` feature. The framework's test harness and the T1 sync→extract→insert→query vehicle.

The `MockConnector` is an in-memory connector whose behaviour is driven entirely by its `config_json`. It emits canned `NormalizedFact`s on a configurable cadence, in both `Polling` and `Push` modes, and can inject failures, panics, and health/auth states to exercise the `ConnectorSupervisor`. It never touches the database — the supervisor inserts its facts through the shared `normalize_and_insert` pipeline.

It lives in `mimir-connectors/src/mock/` and is test-only: the module and its re-exports are gated behind the off-by-default `test-mock-connector` feature, so production builds (including `cargo build -p mimir-connectors --no-default-features`) compile the framework core without the harness. The workspace test run enables the feature through the `mimir` binary's dev-dependencies; a standalone `cargo test -p mimir-connectors --features test-mock-connector` compiles it too.

## Public surface

- `MockConnector` — the configurable connector. `MockConnector::default()` yields the legacy no-op identity (`id "mock"`, type `Email`, `Polling`, health `Online`, empty `extract`) so existing trait tests keep passing.
- `MockConnector::from_config(serde_json::Value) -> Result<Self, ConnectorError>` — build from `config_json` (with the supervisor-injected `__slug` / `__ctype` / `__instance_id`).
- `MockConnector::with_recorder(Arc<MockSyncRecorder>) -> Self` — attach a sync-options observer for F9-style concurrency tests (not part of the config schema or the factory path).
- `MockConnectorFactory` — `ConnectorFactory` that builds a `MockConnector` from its `config_json`; registered under a `(connector_type, backend)` pair.
- `MockFactConfig` — the serde DTO for one canned fact.
- `MockSyncRecorder` — shared observer for `SyncOptions` + in-flight concurrency. `wait_for_completed(count)` awaits actual guard drops, so tests can synchronise on completed cycles without wall-clock sleeps.
- `MockSyncGuard` — RAII guard returned by `MockSyncRecorder::enter`; its `Drop` records the `SyncOptions` and decrements the in-flight counter, so `sync()` tracking stays balanced across returns, panics, and task cancellation.

## Two-step ingestion

`sync()` stages the configured facts into an internal buffer and returns a `SyncOutcome` (item count + cursor); `extract()` drains the buffer into `Vec<NormalizedFact>`. The supervisor then calls `mimir_knowledge::normalize::normalize_and_insert` with a connector `Provenance`, so the mock's facts get the same entity-resolution, confidence, corroboration, and sensitivity gating as any real connector.

## Config schema

```jsonc
{
  "mode": "polling",            // "polling" | "push" (default "polling")
  "interval_ms": 300,           // polling interval, or push internal cadence (ms)
  "jitter_ms": 25,              // polling jitter (ms); ignored in push mode
  "facts": [                    // canned NormalizedFacts emitted by sync()
    {
      "subject": "Alice",
      "subject_type": "Person",          // EntityType; defaults to Concept
      "relationship_type": "works_at",
      "object": "Acme Corp",
      "object_is_entity": false,         // object as literal vs entity reference
      "object_type": "Organization",     // EntityType when object_is_entity
      "valid_from": "2026-01-01T00:00:00Z",
      "valid_until": null,
      "is_sensitive": false,            // producer flag; pipeline narrows it
      "recurrence": "None",             // RecurrenceType; defaults to None
      "requires_user_action": false,
      "raw_reference": "m-1"            // required for connector provenance;
                                        // auto-generated as mock-<slug>-<i> if absent
    }
  ],
  "batch_size": null,           // emit N facts per sync (incremental); omit = all
  "health": "online",           // online|offline|degraded|auth_expired|not_configured
  "auth_state": "Authenticated", // Unauthenticated|Authenticated|Expired
  "fail_first": 0,              // first N sync() calls return Err
  "panic_first": 0,             // first N sync() calls panic
  "always_fail": false,         // every sync() returns Err
  "cursor": null,               // static cursor returned by every successful sync()
  "sync_delay_ms": 0,           // artificial delay inside a successful sync()
  "display_name": null,         // defaults to the slug
  "deletions": ["m-1"]          // raw_references reported by extract_deletions()
}
```

`config_schema()` returns this schema as a `serde_json::Value` for the future `mimir connector add` flow.

## Tombstones (issue #247)

`deletions` lists `raw_reference`s the mock reports as server-side removals via `extract_deletions()` — the mock counterpart of the Calendar connector's CalDAV tombstones. Every `sync()` re-stages the list (a server that keeps re-reporting a tombstone until its cursor advances) and `extract_deletions()` reports it **without draining**; the supervisor acknowledges the processed removals via `acknowledge_deletions()` only after the cycle's trashing, fact insertion, and cursor persistence all succeeded, so a failed cycle re-reports them instead of losing them (PR #313 review). The supervisor trashes the matching KB facts through the shared trash machinery, which is idempotent (re-reports are no-ops). Use it to test the deletion path end-to-end over two cycles: with `batch_size: 1` the first cycle ingests the fact (the same-cycle tombstone trashes nothing yet) and a later cycle's re-staged deletion removes it — a fact and its tombstone staged in the same cycle would leave the fresh insertion in place, because deletions are processed before that cycle's insertions.

## Push mode

Push connectors block inside `sync()` waiting for service events. The mock simulates this by sleeping `interval_ms` at the start of every `sync()` (the "schedule"), then staging the canned facts. The supervisor aborts the runner task on shutdown, cancelling the in-flight sleep. Manual triggers are rejected only for connectors whose mode is *resolved* to push (`TriggerError::PushUnsupported`); an unprobed `auto` connector's trigger is delivered to the runner, which runs the cycle (the mock's `sync` sleeps the interval first, then stages the facts) — the push mock needs no trigger path of its own (issue #475).

## Instance identity

The supervisor injects `__slug`, `__ctype`, and `__instance_id` into a connector's `config_json` before handing it to the factory (see `ConnectorSupervisor::instantiate`). `MockConnector::from_config` reads these to recover its identity (`id()`, `connector_type()`). When `__ctype` is absent it falls back to the legacy no-op identity (`Email`). When `__ctype` is present it must be an integer in range of `i16` and a known `ConnectorType` discriminant; otherwise `from_config` returns `ConnectorError::Config` rather than silently defaulting or wrapping.

## Config validation

`from_config` rejects malformed payloads up front:

- `__ctype`, when present, must be a valid integer `ConnectorType` discriminant (non-integer, out-of-range, and unknown values all yield `ConnectorError::Config`).
- `batch_size`, when present, must be greater than zero. A zero value would let `sync()` succeed forever while fetching no facts, silently defeating ingestion; it is rejected with `ConnectorError::Config`.
- The `facts` array item schema (`config_schema()`) is closed (`additionalProperties: false`) and declares the required fields (`subject`, `relationship_type`, `object`) plus the typed enums for `subject_type` / `object_type` / `recurrence`, matching `MockFactConfig`.
- The `subject_type` / `object_type` / `recurrence` schema `enum` lists and their `default` values are derived from the serde representation of the canonical variant arrays `mimir_knowledge::models::entity::ENTITY_TYPES` and `mimir_knowledge::models::enums::RECURRENCE_TYPES` rather than hand-typed strings, so the schema cannot silently desync from `EntityType` / `RecurrenceType` on a future rename. A regression test asserts the schema values exactly equal the serialised variants.

The `MockSyncRecorder` is cancellation-safe: `enter(options)` returns a `MockSyncGuard` created *before* the first `.await` of `sync()`, and its `Drop` decrements the in-flight counter and records the options. This guarantees the peak-concurrency counter is balanced even when `sync()` is aborted during the push-mode cadence sleep, the `sync_delay`, or unwinds on an injected panic, and that injected failures are recorded rather than omitted.

## Tests

- `tests/mock_connector.rs` — unit tests for config parsing, identity, both modes, canned-fact staging/draining, incremental `batch_size`, knobs, and the recorder.
- `tests/supervisor_lifecycle_tests.rs` — the supervised-lifecycle behavioural suite now drives the `MockConnector` (the previous private `TestConnector` was removed — DRY).
- `tests/mock_ingestion_e2e.rs` — the T1 vehicle: the real `ConnectorSupervisor` + `KnowledgeGraph` ingest a `MockConnector`'s canned facts in both polling and push modes, asserting KB facts + connector provenance (`SourceType::Connector`, `connector_instance_id`, `raw_reference`, `ExtractionMethod::StructuredParse`) and the exact Email reliability-score confidence (0.85).
- `mimir/tests/connector_e2e.rs` — the daemon-level T1 harness (issue #206): the real `mimir` CLI drives an in-process daemon (built with the `mock-connector` feature) through add → auth → resume → sync, then verifies the KB via `mimir kb query` / `kb show --json` — facts land with `source_type=Connector`, provenance tied to the instance, confidence 0.85, and a second instance corroborating the same claim boosts confidence to 0.90 while a plain re-sync stays a re-statement no-op.

## Connections

- Reuses `mimir_knowledge::normalize::NormalizedFact` and `normalize_and_insert` (F4 / #181) — no parallel pipeline.
- Reuses the `ConnectorRegistry` + `ConnectorSupervisor` (F7 / F8) unchanged.
- No new dependencies (in-memory; uses existing `tokio`, `serde`, `chrono`).
- The daemon-level E2E wiring (the broader T1) landed in issue #206 on top of this library-level vehicle.
