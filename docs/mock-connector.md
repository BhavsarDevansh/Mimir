# Mock Connector (Phase 3 F13 / #190)

> **Status:** Done. Always compiled (no feature flag). The framework's test
> harness and the T1 sync→extract→insert→query vehicle.

The `MockConnector` is an in-memory connector whose behaviour is driven entirely
by its `config_json`. It emits canned `NormalizedFact`s on a configurable cadence,
in both `Polling` and `Push` modes, and can inject failures, panics, and
health/auth states to exercise the `ConnectorSupervisor`. It never touches the
database — the supervisor inserts its facts through the shared
`normalize_and_insert` pipeline.

It lives in `mimir-connectors/src/mock.rs` and is always compiled, so
`cargo build -p mimir-connectors --no-default-features` still produces a working
framework + mock harness (the gated Photos/Calendar/Gmail backends are absent,
but the mock and the framework core remain).

## Public surface

- `MockConnector` — the configurable connector. `MockConnector::default()`
  yields the legacy no-op identity (`id "mock"`, type `Gmail`, `Polling`,
  health `Online`, empty `extract`) so existing trait tests keep passing.
- `MockConnector::from_config(serde_json::Value) -> Result<Self, ConnectorError>`
  — build from `config_json` (with the supervisor-injected `__slug` / `__ctype`
  / `__instance_id`).
- `MockConnector::with_recorder(Arc<MockSyncRecorder>) -> Self` — attach a
  sync-options observer for F9-style concurrency tests (not part of the config
  schema or the factory path).
- `MockConnectorFactory` — `ConnectorFactory` that builds a `MockConnector` from
  its `config_json`; registered under a `(connector_type, backend)` pair.
- `MockFactConfig` — the serde DTO for one canned fact.
- `MockSyncRecorder` — shared observer for `SyncOptions` + in-flight concurrency.

## Two-step ingestion

`sync()` stages the configured facts into an internal buffer and returns a
`SyncOutcome` (item count + cursor); `extract()` drains the buffer into
`Vec<NormalizedFact>`. The supervisor then calls
`mimir_knowledge::normalize::normalize_and_insert` with a connector `Provenance`,
so the mock's facts get the same entity-resolution, confidence, corroboration,
and sensitivity gating as any real connector.

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
  "display_name": null          // defaults to the slug
}
```

`config_schema()` returns this schema as a `serde_json::Value` for the future
`mimir connector add` flow.

## Push mode

Push connectors block inside `sync()` waiting for service events. The mock
simulates this by sleeping `interval_ms` at the start of every `sync()` (the
"schedule"), then staging the canned facts. The supervisor aborts the runner
task on shutdown, cancelling the in-flight sleep. F9 manual triggers are
rejected for push connectors (`TriggerError::PushUnsupported`), so the push mock
needs no trigger path.

## Instance identity

The supervisor injects `__slug`, `__ctype`, and `__instance_id` into a
connector's `config_json` before handing it to the factory (see
`ConnectorSupervisor::instantiate`). `MockConnector::from_config` reads these to
recover its identity (`id()`, `connector_type()`). When absent it falls back to
the legacy no-op identity.

## Tests

- `tests/mock_connector.rs` — unit tests for config parsing, identity, both
  modes, canned-fact staging/draining, incremental `batch_size`, knobs, and the
  recorder.
- `tests/supervisor_lifecycle.rs` — the supervised-lifecycle behavioural suite
  now drives the `MockConnector` (the previous private `TestConnector` was
  removed — DRY).
- `tests/mock_ingestion_e2e.rs` — the T1 vehicle: the real
  `ConnectorSupervisor` + `KnowledgeGraph` ingest a `MockConnector`'s canned
  facts in both polling and push modes, asserting KB facts + connector
  provenance (`SourceType::Connector`, `connector_instance_id`,
  `raw_reference`, `ExtractionMethod::StructuredParse`).

## Connections

- Reuses `mimir_knowledge::normalize::NormalizedFact` and
  `normalize_and_insert` (F4 / #181) — no parallel pipeline.
- Reuses the `ConnectorRegistry` + `ConnectorSupervisor` (F7 / F8) unchanged.
- No new dependencies (in-memory; uses existing `tokio`, `serde`, `chrono`).
- Server/HTTP-level E2E wiring (the broader T1) is a separate issue; this
  delivers the library-level vehicle.
