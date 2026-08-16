# Connectors Framework (mimir-connectors)

> **Phase:** 3 — Connectors
>
> **Status:** Scaffolded (issue #178 / F1). Instance registry table + facade landed (issue #179 / F2). `sources` provenance FK migration landed (issue #180 / F3). Shared `normalize_and_insert` boundary landed (issue #181 / F4). Full entity-resolution chain landed (issue #182 / F5). Runtime `Connector` trait + data types landed (issue #183 / F6). `ConnectorRegistry` + multi-backend factory dispatch landed (issue #184 / F7). `ConnectorSupervisor` supervised lifecycle landed (issue #185 / F8). Manual sync triggering landed (issue #186 / F9). Connector secret store landed (issue #187 / F10). Rate limiter + retry/backoff landed (issue #189 / F12). Mock connector test harness landed (issue #190 / F13). Geocoder service + Nominatim backend landed (issue #191 / S1). Entity-locations write path + geocode wiring landed (issue #193 / S3). Entity-locations proximity query landed (issue #194 / S4). **First concrete backend landed: Photos local-filesystem connector (issue #195 / C1).** The supervisor now injects the persisted `sync_cursor` into connector `config_json` as `__cursor` so incremental connectors (Photos) can skip already-processed files across restarts. The daemon `AppState` wiring, `connector` CLI (A1–A3), optional OS-keyring backend (#188), the event → KB fact extraction + write-back (Calendar C4 / #198), the email backends (C5–C7) remain to be built. **The CalDAV Calendar transport backend landed (issue #197 / C3):** a `CalDavClient` (PROPFIND + sync-collection REPORT, sync-token incremental sync) + `CalendarConnector` (Polling) with app-password and OAuth-refresh auth via the secret store, parsing VEVENTs with `icalendar`; `extract()` is transport-only (C4 owns event → fact extraction). This wired the `SecretStore` into `ConnectorContext` (`with_secret_store`).
>
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`

## Purpose

`mimir-connectors` is the service ingestion framework for Mimir. Connectors are background sync workers that fetch data from external services (email, calendar, photos, …), normalize it, and insert it into the knowledge graph through the *existing* fact pipeline — the same `normalize_and_insert` boundary used by conversational `remember` calls. They are not a parallel track.

## Database-access boundary

Connectors never hold a `sqlx` pool handle. All persistence goes through the [`mimir_knowledge::KnowledgeGraph`] facade. Accordingly, `mimir-connectors` depends on `mimir-core` and `mimir-knowledge` **only** and does **not** declare a direct `sqlx` dependency (it enters the build graph only transitively, via `mimir-knowledge`'s internal use).

## Shared ingestion boundary (F4 / #181)

The resolve → confidence → sensitivity-gate → insert orchestration lives in `mimir-knowledge::normalize` as a single reusable function, so connector ingestion and conversational `remember` extraction share one deterministic Rust pipeline:

```rust
pub async fn normalize_and_insert(
    kg: &KnowledgeGraph,
    facts: Vec<NormalizedFact>,
    provenance: Provenance,
) -> Result<ExtractionOutcome, KnowledgeError>
```

- **`Provenance`** (batch-level) carries the connector instance id + type and the `extraction_method`. A connector sync calls this once per batch with a `Provenance::connector(instance_id, connector_type, method)`.
- **`NormalizedFact`** (per-fact) carries typed content (entity types, parsed temporal bounds, typed recurrence, validated category ids, sensitivity flag, optional correction scope) and the per-fact `raw_reference` (the native item id). `source_type` is per-fact (`Connector` for connector facts).
- **`connector_fact`** (`mimir_connectors::fact`, issue #255) is the single constructor for connector facts: it fills the fixed defaults (`source_type: Connector`, non-sensitive, non-correction, no category ids, no user action) and takes the per-shape fields (subject, relationship, object, entity-ness, temporal bounds, recurrence, raw reference, extraction method, event-type hint, location overlay) as arguments. The Photos, iCal VEVENT (Calendar + Email iMIP), and Email JSON-LD backends all funnel through it, so a new connector cannot silently drift on a default.
- **Confidence** is the per-source-type / per-connector reliability score read from the `connector_reliability` table (`confidence::connector_reliability`, seeded defaults as fallback) with no extraction-method discount. Corroboration / supersession / inference are inherited from `insert_fact_in_tx`, so cross-connector corroboration (Gmail flight + Calendar event on overlapping dates) is an explicit acceptance criterion, not an accident.

Because `mimir-connectors` depends on `mimir-knowledge`, it reaches these types directly; it never needs a parallel insert path.

## Connector trait + data types (F6 / #183)

The runtime `Connector` trait is the contract every service-ingestion worker implements. It is `#[async_trait]` with a `Send + Sync` supertrait so it is object-safe as `Arc<dyn Connector>` (native `async fn` in traits is not dyn-compatible; `async-trait` is required). Each trait object represents one configured connector *instance* (one row in the `connectors` table).

### Ingestion model

Ingestion is a **two-step, DB-free** process owned by the connector, with the *supervisor* (F8) performing the database insert:

1. `sync(SyncOptions) -> SyncOutcome` — fetches raw items from the service into the connector's own internal buffer. Returns the item count and an updated sync cursor. Raw types stay connector-internal (no generic `RawEvent`).
2. `extract() -> Vec<NormalizedFact>` — drains the buffer into typed, parsed facts. Entity *types* are set; entity *ids* are **not** resolved here.
3. The supervisor builds `Provenance::connector(instance_id, type, method)` and calls `mimir_knowledge::normalize::normalize_and_insert`, which resolves entities (F5 chain), assigns confidence, runs the sensitivity gate, and inserts (inheriting corroboration / supersession / inference).

Because the connector never touches the database, the trait takes **no `&KnowledgeGraph`** parameter. This keeps the crate `sqlx`-free and makes connectors unit-testable without a live knowledge graph (F13 mock).

Every method takes `&self` (matching the workspace `Tool` trait), so the whole surface is callable through the shared `Arc<dyn Connector>` storage used by the registry (F7) and supervisor (F8). A connector that needs to mutate internal state (raw-item buffer, sync cursor, cached auth state) owns that state behind interior mutability (e.g. `tokio::sync::Mutex`) inside its concrete type — the trait surface itself stays shared-reference friendly and needs no storage-layer `Mutex<dyn Connector>`.

### Trait surface

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    fn id(&self) -> &str;                         // instance slug
    fn name(&self) -> &str;                        // display name
    fn connector_type(&self) -> ConnectorType;     // provenance + reliability axis
    fn mode(&self) -> ConnectorMode;               // Polling { interval, jitter } | Push
    fn config_schema(&self) -> serde_json::Value;

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError>;
    async fn health(&self) -> Result<HealthStatus, ConnectorError>;
    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError>;
    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError>;
    fn durable_state(&self) -> Option<String> {
        // default: None — no connector-side durable state to persist
        None
    }
    fn durable_state_persisted(&self) {
        // default: no-op — no durable state to acknowledge
    }
    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        // default: empty — server-side removals to trash in the KB
        Ok(Vec::new())
    }
    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        // default: no-op — nothing buffered
        let _ = deleted;
        Ok(())
    }
    async fn on_cycle_succeeded(&self, new_cursor: Option<&str>) {
        // default: no-op — cursor adoption is per-connector (issue #314)
        let _ = new_cursor;
    }
    async fn act(&self, action: ConnectorAction)   // default: UnsupportedAction
        -> Result<ActionResult, ConnectorError>;
    async fn forget(&self) -> Result<(), ConnectorError>;
}
```

- **`durable_state`** (issue #262) is the generic hook for connector-owned state that must survive a daemon restart. After each successful extraction cycle the supervisor persists the returned opaque string via `KnowledgeGraph::update_sync_progress_and_durable_state` (the `connectors.durable_state` column) and re-injects it at construction as the `__durable_state` config key — the same read/write pair as `__cursor` / `update_sync_cursor`, for state that is not sync progress. `None` means "unchanged since the last persist" (no write). The Email connector uses it for its bounded, durable LLM-extraction retry ledger; connectors that keep no durable state leave the default. The returned value is **not consumed**: the supervisor calls `durable_state_persisted()` only after the combined database commit succeeds, so a failed write leaves the connector's state dirty and the next cycle re-persists it — a write failure can never silently lose durable state.

- **`authenticate`** takes no arguments: credentials are injected at construction by the factory (F7) / secret store (F10), per decision D′ (which also injects `Arc<dyn LlmBackend>`). It returns the resulting `ConnectorAuthState` for the supervisor to persist.
- **`act`** is optional write-back with a default implementation returning `ConnectorError::UnsupportedAction`; backends that support write-back (e.g. Calendar event creation in C4) override it.
- **`extract_deletions`** (issue #247) is the server-side deletion (tombstone) report: the supervisor calls it every cycle after `extract()` and trashes the returned `raw_reference`s via `KnowledgeGraph::forget_connector_facts_by_raw_reference` (shared trash machinery, idempotent, instance-scoped). The report is **non-destructive** — the supervisor calls **`acknowledge_deletions`** only after the cycle's trashing, fact insertion, and cursor persistence all succeeded, so a failed cycle re-reports the same removals instead of losing them (PR #313 review). The defaults return an empty set / a no-op; the Calendar connector overrides both with its staged CalDAV tombstones, and the Email connector overrides both with its staged iMIP `CANCEL` VEVENT UIDs (issue #283).
- **`on_cycle_succeeded`** (issues #314, #332) is the failure-safe cursor-adoption hook: the supervisor calls it (default no-op) after a cycle fully succeeded — extraction, trashing, fact insertion, and cursor/durable-state persistence all committed — with the persisted `SyncOutcome::new_cursor`, so the connector may adopt it as its in-memory progress marker. Connectors must **not** advance an in-memory cursor inside `sync` (unless they re-deliver failed windows by other means, e.g. a durable retry ledger): the persisted cursor advances only on success, so an earlier adoption would skip a failed cycle's window on the next in-process cycle. The Calendar, Email, and Photos connectors all implement it — their `sync` no longer advances the in-memory cursor. The Email connector's durable retry ledger (issue #262) only covers LLM-layer failures inside `extract`, so a hard extract/insert/persist failure still needs the un-advanced cursor to re-fetch the failed window; in IDLE (push) mode the Email connector additionally skips the IDLE wait and re-fetches from the last confirmed cursor on the next in-process cycle, because the IDLE notification for the failed window will not re-fire, and re-fetches are deduped against the staged buffer so re-staged LLM retries are never duplicated; the Photos connector (push mode) re-scans the watch directory from the last confirmed cursor on the next in-process cycle, because the file watcher does not re-deliver consumed events. Connectors whose progress lives solely in the persisted column leave the default.
- **`forget`** handles connector-local cleanup; the supervisor additionally cascades the deletion to knowledge-graph facts with this `connector_instance_id` via the existing trash machinery.

### Data types

| Type | Purpose |
|------|---------|
| `ConnectorMode` | `Polling { interval, jitter }` (supervisor-polled) or `Push` (IMAP IDLE / file watcher). |
| `SyncOptions` | `full: bool` (ignore cursor) + optional `since: Option<Duration>` time-window hint. The opaque incremental cursor lives in `connectors.sync_cursor`, not here. |
| `SyncOutcome` | `fetched: u32`, `new_cursor: Option<String>` (`Some` advances/clears the cursor, `None` = unchanged), `fetched_at: DateTime<Utc>`. |
| `HealthStatus` | Transient probe: `Online` / `Offline` / `Degraded` / `AuthExpired` / `NotConfigured`. |
| `ConnectorAction` / `ActionResult` | Write-back request (`kind` + JSON `payload`) and outcome (`success`, `native_id`, `message`). |
| `ConnectorError` | `thiserror` enum: `Authentication`, `NotAuthenticated`, `Network`, `Config`, `Parse`, `UnsupportedAction`, `Io`, `BackendNotFound`, `BackendAlreadyRegistered`, `Other`. Does not wrap `KnowledgeError` (the connector does not insert). |

### `HealthStatus` vs persisted lifecycle

`HealthStatus` is a **transient runtime probe** (is the service reachable and authenticated *right now*), deliberately renamed to disambiguate from the persisted enums `ConnectorStatus` (`Setup`/`Active`/`Paused`/`Error`) and `ConnectorAuthState` (`Unauthenticated`/`Authenticated`/`Expired`). The supervisor calls `health()` and maps the probe onto the persisted columns — e.g. `AuthExpired` → `auth_state = Expired`, `status = Paused`; `Offline` → `status = Error`.

## Multi-backend registry + factory dispatch (F7 / #184)

`ConnectorRegistry` maps `(connector_type, backend)` to a `ConnectorFactory`. A connector **type** (`Gmail` / `Calendar` / `Photos` / …) is the provenance and reliability axis — fixed and seeded. A **backend** (`imap`, `caldav`, `local-fs`, …) is the provider implementation chosen per instance and persisted as the `backend` column on `connectors` (F2). Adding a new backend is a new `register` call — no schema change.

### API

```rust
pub trait ConnectorFactory: Send + Sync {
    fn create(
        &self,
        config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError>;
}

pub struct ConnectorRegistry { /* RwLock<HashMap<(ConnectorType, String), Arc<dyn ConnectorFactory>>> */ }

impl ConnectorRegistry {
    pub fn new() -> Self;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn register<F: ConnectorFactory + 'static>(
        &self, connector_type: ConnectorType, backend: impl Into<String>, factory: F,
    ) -> Result<(), ConnectorError>;
    pub fn register_arc(
        &self, connector_type: ConnectorType, backend: impl Into<String>,
        factory: Arc<dyn ConnectorFactory>,
    ) -> Result<(), ConnectorError>;
    pub fn is_registered(&self, connector_type: ConnectorType, backend: &str) -> bool;
    pub fn factory(&self, connector_type: ConnectorType, backend: &str)
        -> Option<Arc<dyn ConnectorFactory>>;
    pub fn backends_for(&self, connector_type: ConnectorType) -> Vec<String>;
    pub fn registered_types(&self) -> Vec<ConnectorType>;
    pub fn pairs(&self) -> Vec<(ConnectorType, String)>;
    pub fn create(
        &self, connector_type: ConnectorType, backend: &str, config: serde_json::Value,
    ) -> Result<Arc<dyn Connector>, ConnectorError>;
    pub fn create_with_context(
        &self, connector_type: ConnectorType, backend: &str, config: serde_json::Value,
        ctx: &ConnectorContext,
    ) -> Result<Arc<dyn Connector>, ConnectorError>;
}
```

`pairs()` lists every registered `(connector_type, backend)` pair, sorted by type then backend (wire-string form) so output is deterministic — it backs the daemon's `GET /connectors/catalog` discovery route and `mimir connector catalog` (issue #271). `backends_for` / `registered_types` predate it and return unordered results.

`ConnectorContext` carries the shared services injected at construction: an optional `Arc<dyn Geocoder>` (Photos reverse geocoding, C2 / #196), an optional `Arc<dyn SecretStore>` (Calendar / Email credentials, F10 / #187), the canonical user identity name (A1), and an optional `Arc<dyn LlmBackend>` (Email LLM extraction, C7 / #201, routed through the shared pool's system queue per decision D′). `create_with_context` forwards the context to the factory; `create` is the config-only convenience path that passes `ConnectorContext::empty()`.

### Design notes

- **Concurrency.** Following the workspace `ToolRegistry` / `SkillRegistry` pattern, registration uses interior mutability (`RwLock`) with a `&self` receiver, so a registry shared in `AppState` behind `Arc` can be populated at startup and queried concurrently at runtime.
- **Fail-loud duplicates.** `register` returns `ConnectorError::BackendAlreadyRegistered` on a repeated `(type, backend)` rather than silently shadowing the existing factory. `register_arc` is the pre-built-`Arc` variant.
- **Dispatch errors.** `create` / `create_with_context` return `ConnectorError::BackendNotFound` when no factory is registered for the requested pair.
- **`FnConnectorFactory`.** A closure-backed `ConnectorFactory` (`new<F: Fn(serde_json::Value, &ConnectorContext) -> Result<…> + Send + Sync + 'static>`) for simple backends and tests. `MockConnectorFactory` (always-compiled, F13 / #190) produces `MockConnector`s from their `config_json`, keeping the registry exercisable under every feature combination, including `--no-default-features`. The mock is fully configurable (mode, cadence, canned facts, health/auth, failure/panic injection) and is the T1 sync→extract→insert→query vehicle.
- **Reliability stays per-type.** Confidence for connector facts is the `connector_reliability` table score for the type (via `confidence::connector_reliability`, seeded defaults as fallback), keyed on the type axis only. The registry never branches reliability on `backend`; an instance reports the same `connector_type()` regardless of which backend constructed it.
- **Construction context.** The factory signature takes the construction context directly (the Phase 3 plan's decision D′ anticipated this extension; it landed with C2 / #196 and F10 / #187 rather than remaining a forward-looking note). The supervisor (F8) is the only production constructor: it calls `create_with_context` with the daemon-wide context built in `mimir-server` (`with_secret_store` / `with_geocoder` / `with_user_identity` / `with_llm_backend`). The config-only `create` is the convenience path for callers with no services to inject (tests, and any non-supervisor construction). Backends that need no shared services ignore `ctx`.

## ConnectorSupervisor — supervised lifecycle (F8 / #185)

`ConnectorSupervisor` owns one supervised tokio task per *active* connector instance and centralises spawn, restart-with-backoff, circuit breaker, startup restore, graceful shutdown, cursor persistence, and durable-state persistence. It is the caller that runs the two-step ingestion model end to end: `health` -> `sync` -> `extract` -> `normalize_and_insert`, then `update_sync_progress_and_durable_state` (the cursor and durable state commit in one transaction), then `Connector::on_cycle_succeeded` so the connector adopts the persisted cursor only after the cycle fully succeeded (issue #314). All status / auth / cursor / durable-state writes go through the `KnowledgeGraph` facade — the supervisor never holds a `sqlx` pool, keeping the `sqlx`-free crate boundary intact.

### Construction

```rust
pub struct SupervisorConfig {
    pub max_failures: u32,   // consecutive failures before the breaker trips
    pub base_backoff: Duration,
    pub max_backoff: Duration,
}

pub struct ConnectorSupervisor { /* registry, kg, config, shutdown rx, tasks */ }

impl ConnectorSupervisor {
    pub fn new(registry: Arc<ConnectorRegistry>, kg: Arc<KnowledgeGraph>,
               config: SupervisorConfig, shutdown: watch::Receiver<bool>) -> Self;
    pub async fn restore(&self) -> Result<usize, SupervisorError>; // spawn Active rows
    pub async fn shutdown(&self);                                   // abort + join
    pub async fn running_count(&self) -> usize;
    pub async fn is_running(&self, id: i32) -> bool;
}
```

`SupervisorConfig` is injected at construction (no environment mutation, per the safety policy). Backoff is deterministic in V1 — `base_backoff * 2^(n-1)`, capped at `max_backoff`; randomised jitter / rate-limit primitives belong to F12 (#186) and are not duplicated here.

### Startup restore

`restore` loads the `connectors` table and spawns a runner for every row whose `status == Active`. `Paused`, `Error`, and `Setup` rows are left down (not auto-spawned). Rows whose `(type, backend)` has no registered factory, or whose `config_json` is invalid, are logged and skipped — one bad connector never aborts startup. Returns the number of tasks spawned.

Before handing each row's `config_json` to the factory, the supervisor injects `__slug`, `__ctype`, `__instance_id`, `__cursor`, and `__durable_state` so the connector instance knows which row it represents and can seed its incremental sync cursor and any connector-owned durable state. This is the V1 mechanism for passing instance identity and progress through the factory's `config` argument; the shared-services `ConnectorContext` (geocoder, secret store, user identity, LLM backend) is injected separately via the supervisor's `create_with_context` call. The `__cursor` injection (C1 / #195) is the read side that complements `KnowledgeGraph::update_sync_cursor` (the write side): it lets an incremental connector — e.g. the Photos file watcher — skip already-processed files across restarts. The `__durable_state` injection (issue #262) is the read side that complements `KnowledgeGraph::update_sync_progress_and_durable_state` (the write side, committed atomically with the cursor): the Email connector seeds its bounded LLM-extraction retry ledger from it so pending retries and terminal failures survive restarts. `None` values are injected as JSON `null` (a full first scan / no durable state).

### Per-connector runner loop

Each runner performs an initial `authenticate()` handshake (a failed handshake pauses the connector and exits), then loops:

1. Check the shared `watch::Receiver<bool>` shutdown signal; exit if set.
2. Run one cycle in an **isolated sub-task** (`tokio::spawn`) so a connector panic surfaces as a `JoinError::is_panic` rather than unwinding the runner. The cycle and the shutdown signal race in a `tokio::select!`; if shutdown wins, the cycle's `AbortHandle` cancels the in-flight work and the runner exits.
3. Classify the cycle result and act:
   - **Ok** — reset the failure count, persist sync progress, then clear `last_error` (`set_connector_status(Active, Some(None))`). The cursor and connector-side durable state (`Connector::durable_state`, issue #262) are persisted **atomically** in one transaction via `update_sync_progress_and_durable_state`, so a crash between the two writes cannot advance the cursor without its retry ledger (PR #318 review). When `new_cursor` is `Some`, the cursor is advanced/cleared; when `None` (unchanged), only `last_sync_at` is stamped, preserving the existing progress token. The connector acknowledges the persist (`durable_state_persisted`) only after the combined commit succeeds, and adopts the persisted cursor via `Connector::on_cycle_succeeded` (issue #314) — never inside `sync` — so a failed cycle re-syncs from the last confirmed cursor on the next in-process cycle.
   - **Err / Panic** — increment failures, write `last_error` (status stays `Active`), exponential backoff; once failures reach `max_failures`, move to `Error` and stop auto-restarting (manual `resume` required).
   - **AuthExpired** (from `health`) — `set_auth_state(Expired)` + `set_connector_status(Paused, ...)`, then exit (not auto-restarted).
4. For `Polling` connectors, sleep the declared `interval + jitter` (cancellable by shutdown) before the next cycle. `Push` connectors block inside `sync` and loop immediately.

### Graceful shutdown + cursor persistence

The shared `watch::Receiver<bool>` is the same shutdown channel the daemon already uses for OS signals and `/stop`, so a single `mimir stop` drains every runner. Because the cursor is persisted after every *successful* `sync`, the cursor always reflects the last completed sync — `mimir stop` mid-cycle aborts the in-flight cycle (no cursor advance) and the next restart resumes from the last persisted cursor, re-fetching at most the abandoned batch. `shutdown()` signals every runner and awaits its graceful exit (each runner aborts and awaits its in-flight cycle before returning); runners still alive after a grace period are aborted, and the cycle registry is drained and awaited so no in-flight cycle task outlives `shutdown` (issue #266).

### Lifecycle control: `start` / `pause` / `resume` (A2 / #203, hardened in #266)

`ConnectorSupervisor::start(id)` (re-spawn one runner, used by `resume`), `pause(id)`, and the daemon's forget cascade and `DELETE /connectors/{id}` route are **lifecycle mutations** and are serialised per connector by a `lifecycle_lock` — a `Mutex<()>` keyed by instance id, created on first use and retained. `start` / `resume` hold it across the whole stop → instantiate → spawn sequence, `pause` holds it across stop → status-write, and `DELETE` holds it across stop → row deletion, so concurrent lifecycle calls for one instance queue instead of racing: a re-spawn never leaks a runner task, a concurrent `pause` + `start` can never leave a `Paused` row with a live runner, and a `DELETE` can never delete a row out from under a freshly spawned runner (issue #266).

`stop(id)` is graceful: it signals the runner over a per-runner `watch` channel and awaits its termination. The runner aborts and awaits its in-flight cycle sub-task before exiting — and its auth handshake is preemptable by the stop signal too, so `stop` never waits on a slow or hung network handshake — so nothing outlives `stop`: a stopped connector cannot keep syncing or writing facts, and a re-spawn's first cycle never overlaps the previous runner's last cycle. A `Drop` guard on the cycle's `AbortHandle` covers the abort fallback path (`shutdown()`), so an aborted runner still cancels its in-flight cycle instead of detaching it. `shutdown()` goes one step further: runners are signalled first and awaited (the graceful path), and stragglers are aborted only after a grace period — with each cycle's `JoinHandle` retained in a registry and awaited afterwards, so even the abort path cannot leave a cycle task running after `shutdown` returns (issue #266).

### `yield-on-user-activity` deferred

Decision D of the Phase 3 plan calls for connectors to yield to user activity. This is **deferred for V1** (`last_user_activity` is not consulted yet); it lands with the proactive-agent / scheduling work.

### Server wiring (forward-looking)

F8 lands the supervisor as a library component in `mimir-connectors` with unit/integration tests against a configurable in-memory mock. Daemon `AppState` wiring (owning a `ConnectorSupervisor`, calling `restore()` after KG/LLM are up, and `shutdown()` in the graceful-drain path) and the `mimir connector ...` CLI subcommands are separate Phase 3 issues (A1-A3) that depend on F8.


## Manual sync triggering (F9 / #186)

`ConnectorSupervisor::trigger_sync(id, SyncOptions)` wakes a connector's runner from its polling-interval wait so a sync runs immediately with the caller-supplied options, instead of waiting for the next interval. A slug-based `trigger_sync_by_slug` resolves the instance id via the knowledge graph first.

### Mechanism

- Each active connector owns a one-permit `tokio::sync::Semaphore` and a per-connector request channel (`mpsc`) into its runner.
- `trigger_sync` acquires the permit (serialising concurrent callers — overlapping triggers queue rather than launching parallel cycles), sends a `TriggerRequest { options, reply }`, and awaits the cycle's outcome.
- The runner's post-cycle wait is a `select!` between the polling interval, a trigger request, and shutdown. A trigger preempts the interval; the cycle then runs with the trigger's `SyncOptions` and replies with a `TriggerOutcome`. Backoff after a failed cycle is likewise preemptable by a trigger.
- `run_cycle` takes `SyncOptions` (so `full` / `since` reach `Connector::sync`); `CycleOutcome::Ok` carries the `SyncOutcome` so a triggered cycle can report `fetched` / `new_cursor` back to the caller.

### `SyncOptions`

| Field | Manual-trigger meaning |
|-------|------------------------|
| `full` | `true` forces a non-incremental pass — the connector ignores/resets its persisted cursor and re-fetches everything. |
| `since` | Optional relative window (`now - since`) restricting fetched items. |

`SyncOptions::default()` is an incremental sync with no window — the same options an automatic polling cycle uses.

### Outcomes and errors

`trigger_sync` returns `Result<TriggerOutcome, TriggerError>`:

- `TriggerOutcome::Ok { fetched, new_cursor }` — the cycle succeeded.
- `TriggerOutcome::AuthExpired` — the service rejected credentials; the supervisor has already paused the connector.
- `TriggerOutcome::Failed(msg)` — a recoverable cycle error (panic, offline, parse failure, shutdown mid-cycle).
- `TriggerError::NotFound` / `NotFoundSlug` — no connector row with that key.
- `TriggerError::NotRunning` — the connector is `Paused` / `Error` / `Setup` or its runner has exited (resume it first).
- `TriggerError::PushUnsupported` — push-mode connectors have no polling interval to preempt; push manual sync is deferred to a later Phase 3 issue.
- `TriggerError::RunnerDropped` — the runner stopped mid-sync before reporting.

The issue spec described the mechanism as a per-connector `tokio::sync::Notify` plus a serialisation semaphore. The implementation uses a small request channel (carrying the `SyncOptions` and returning the outcome) instead of a bare `Notify`, because `--full` / `--since` must reach the cycle and the future HTTP route wants the sync result — but the one-permit semaphore is kept as the explicit serialisation gate, matching the spec's intent ("no concurrent sync on the same connector").

### Wiring (forward-looking)

F9 lands the trigger as a library API on `ConnectorSupervisor` in `mimir-connectors`, with integration tests against a configurable in-memory mock. The `mimir connector sync <slug> [--full|--since]` CLI command and its HTTP route are separate Phase 3 issues (A2 action routes / A3 CLI) that call `trigger_sync` / `trigger_sync_by_slug` once the supervisor is wired into `AppState` (A1).

## Crate layout

The crate root (`src/lib.rs`) re-exports the public API; each subsystem is a directory with one file per concern. `src/supervisor/` splits lifecycle config, errors, trigger types, the runner (struct + spawning), runtime control (start/stop/pause/resume/trigger dispatch), and the per-connector cycle loop across `config.rs`, `error.rs`, `trigger.rs`, `runner.rs`, `control.rs`, and `cycle.rs`. `src/calendar/` splits `construct`, `credentials`, `sync`, `trait_impl`, and `payload` with the CalDAV transport under `caldav/{client,ical,xml}`. `src/email/` splits `config`, `factory`, `imap`, the connector (`connector/{construct,credentials,extract,session,trait_impl}`), JSON-LD extractors (`jsonld/{facts,html,nodes,reservations,values}`), and the LLM extractor (`llm/{message,parse,schema}`). `src/secrets/` splits `error`, `bundle`, the `SecretStore` trait + slug validation (`store.rs`), the on-disk store (`file.rs`), and the in-memory helper (`memory.rs`). `src/oauth/` splits the `OAuthHttpClient` adapter (`http_client.rs`) from the refresh grant + endpoint gate + error mapping (`refresh.rs`). `src/rate_limit/`, `src/geocoder/`, `src/ical/`, `src/mock/`, and `src/photos/` follow the same pattern. `src/fact.rs` is a single-file module owning the shared connector `NormalizedFact` constructor (`connector_fact`, issue #255), so every backend builds facts through one helper.

| Module | Role | Filled by |
|--------|------|-----------|
| `connector` | Runtime `Connector` trait + data types (`ConnectorMode`, `SyncOptions`, `SyncOutcome`, `HealthStatus`, `ConnectorAction`, `ActionResult`, `ConnectorError`) and the `ConnectorFactory` trait | F6 — done (#183) |
| `registry` | `ConnectorRegistry` + multi-backend factory dispatch: `(connector_type, backend)` → `ConnectorFactory`, plus the closure-backed `FnConnectorFactory` | F7 — done (#184) |
| `supervisor` | `ConnectorSupervisor` + `SupervisorConfig` + `SupervisorError` + `TriggerOutcome` + `TriggerError`: supervised per-connector task lifecycle (spawn / restart / backoff / circuit-breaker / startup-restore / graceful-shutdown / cursor-persistence), and manual sync triggering (`trigger_sync` / `trigger_sync_by_slug` — per-connector semaphore + request channel; preempts the polling interval) | F8 — done (#185), F9 — done (#186) |
| `mock` | `MockConnector` + `MockConnectorFactory` + `MockFactConfig` + `MockSyncRecorder` (configurable, always-compiled test harness: emits canned `NormalizedFact`s in `Polling`/`Push` modes with health/auth/failure/panic injection and sync-options observation) | F13 — done (#190) |
| `photos` *(feature `photos`)* | `PhotosConnector` + `PhotosConnectorFactory` + `PhotosCursor`: read-only local-filesystem push connector — `notify` recursive watcher + `kamadak-exif` GPS/datetime extraction + per-file mtime/inode incremental cursor | C1 — done (#195) |
| `oauth` *(feature `oauth`)* | `OAuthHttpClient` (the `oauth2`-crate `AsyncHttpClient` adapter over the workspace reqwest 0.13 client) + `refresh_token` / `resolve_access_token` refresh helpers with the HTTPS/loopback endpoint gate and secret-hygiene error mapping; used by the Calendar and Email OAuth backends, and reserved for the A4 CLI PKCE flow (not yet implemented) | #240 — done (v0.96.0) |

Provenance types that connectors reference (`ConnectorType`, `SourceType`) live in `mimir-knowledge` and are re-used, not duplicated (DRY).

## Feature flags

```toml
[features]
default = ["photos", "calendar", "gmail"]
photos = ["dep:notify", "dep:notify-debouncer-full", "dep:kamadak-exif"] # local photo ingestion (C1–C2 done)
oauth = ["dep:oauth2", "dep:http"] # shared OAuth 2.0 client + refresh (issue #240)
calendar = ["dep:icalendar", "dep:roxmltree", "dep:chrono-tz", "dep:uuid", "oauth"] # CalDAV calendar ingestion (C3–C4 done)
gmail = ["dep:async-imap", "dep:base64", "dep:tokio-rustls", "dep:rustls", "dep:rustls-native-certs", "dep:futures", "dep:icalendar", "dep:chrono-tz", "dep:mail-parser", "oauth"] # IMAP email ingestion (C5–C7 done)
```

The framework core and the mock connector are **always built**. Running `cargo build -p mimir-connectors --no-default-features` therefore still compiles a working framework + mock harness — the gated backends are simply absent. The `photos` feature gates the `notify` / `notify-debouncer-full` / `kamadak-exif` dependencies and the `photos` module (C1 / #195); `oauth` gates `oauth2` / `http` and the `oauth` module (#240); `calendar` and `gmail` gate their backend deps and modules and enable `oauth` for their OAuth refresh path.

## Workspace wiring

- `mimir-connectors` is a workspace `members` entry.
- `mimir-server` depends on `mimir-connectors`; the daemon will own a `ConnectorRegistry` and `ConnectorSupervisor` once A1 wires them into `AppState` (not yet done in F1).

## Safety

`#![deny(unsafe_code)]` is enforced at the crate root, consistent with the workspace-wide no-`unsafe` guarantee.

## Connector instance registry (F2)

Issue #179 added the `connectors` instance-registry table (migration `042_create_connectors.sql`) plus its Rust model, queries, and `KnowledgeGraph` facade methods. Each row is a single configured connector instance — one Gmail account, one CalDAV calendar — so backends can persist sync cursor, auth state, and health across daemon restarts.

### Schema

```sql
CREATE TABLE connectors (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  connector_type_id INTEGER NOT NULL REFERENCES connector_types(id),
  slug TEXT NOT NULL UNIQUE,
  backend TEXT NOT NULL,
  display_name TEXT NOT NULL,
  config_json TEXT NOT NULL,
  status_id INTEGER NOT NULL DEFAULT 1 REFERENCES connector_statuses(id),
  auth_state_id INTEGER NOT NULL DEFAULT 1 REFERENCES connector_auth_states(id),
  sync_cursor TEXT,
  durable_state TEXT,
  last_sync_at TIMESTAMP,
  last_error TEXT,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

Two lookup tables mirror the `event_statuses` pattern: `connector_statuses` (`Setup=1`, `Active=2`, `Paused=3`, `Error=4`) and `connector_auth_states` (`Unauthenticated=1`, `Authenticated=2`, `Expired=3`). The Rust enums `ConnectorStatus` and `ConnectorAuthState` (`#[repr(i16)]`, `sqlx::Type`) live in `mimir-knowledge/src/models/enums.rs`. This deliberately uses typed integer enums rather than the `TEXT` columns proposed in the issue, to match the rest of the knowledge-graph schema and the project's "smallest data type" rule.

> The `sources.connector_instance_id` provenance FK and the `SELECT COUNT(*) FROM sources WHERE connector_instance_id = ?` item-count query landed in **F3 (#180)**: see [Sources provenance FK (F3)](#sources-provenance-fk-f3).

### Facade methods

`KnowledgeGraph` exposes: `list_connectors`, `get_connector_by_slug`, `get_connector` (by id), `upsert_connector`, `update_sync_cursor`, `update_durable_state`, `update_sync_progress_and_durable_state`, `touch_last_sync`, `set_connector_status`, and `set_auth_state`.

- `upsert_connector` is keyed on `slug`. `slug` and `connector_type` are immutable identity: on conflict it overwrites the mutable config surface (`backend`, `display_name`, `config_json`, `status`, `auth_state`) and bumps `updated_at`; it **preserves** `id`, `created_at`, and the sync-progress fields (`sync_cursor`, `durable_state`, `last_sync_at`, `last_error`), which are owned by their dedicated mutators. Reusing an existing `slug` with a different `ConnectorType` returns `KnowledgeError::ConnectorTypeMismatch` rather than silently rewriting the instance's kind (which would leave the previous backend's type-specific sync state attached to a different connector type). The check is atomic: the `ON CONFLICT DO UPDATE ... WHERE connectors.connector_type_id = excluded.connector_type_id` guard updates zero rows on a mismatch, so `RETURNING` is empty and a clean error is surfaced.
- `set_connector_status` takes an `Option<Option<String>>` `error` argument: `None` leaves `last_error` untouched, `Some(None)` clears it to NULL, and `Some(Some(msg))` records `msg` (e.g. a circuit-breaker reason).
- Unknown ids return `KnowledgeError::ConnectorNotFound`; a slug reused with a different type returns `KnowledgeError::ConnectorTypeMismatch`. The `connector_type` field is the typed `ConnectorType` enum (variants map to seeded `connector_types` rows), so the `connector_types(id)` foreign key is the DB-level guard and no separate type-existence validation is needed.

## Sources provenance FK (F3)

Issue #180 migrated `sources.connector_id TEXT` to `connector_instance_id INTEGER REFERENCES connectors(id)` (migration `043_sources_connector_instance_fk.sql`). SQLite cannot change a column type in place, so this is the standard table-rebuild dance; it is lossless for existing DBs because legacy `sources.connector_id` values were already limited to `NULL` or `''` (the insert paths differ — `queries/source.rs` normalised a missing connector to `''`, `queries/fact.rs` bound `NULL`), and both map to `connector_instance_id IS NULL`. `connector_type_id` is retained (denormalised) so the confidence model can read the connector kind without a join even when `connector_instance_id` is `NULL`.

The rebuild restores the NULL-aware unique index as `(fact_id, source_type_id, COALESCE(connector_instance_id, 0), COALESCE(raw_reference, ''))` — `0` is a safe sentinel because autoincrement ids start at `1` — plus a new `idx_sources_instance` index for the item-count query.

### Rust model + validation gate

`Source.connector_instance_id: Option<i32>` and `NewFact.connector_instance_id: Option<i32>` replace the old `Option<String>` labels; every insert site (`extract.rs`, `queries/fact.rs` corroboration + `insert_fact_in_tx`, `queries/trash.rs` restore, `optimization/mod.rs` dedup-merge) was updated.

The `insert_fact` confidence gate now keys connector provenance off `connector_instance_id` rather than `connector_type`. When set it requires `raw_reference` and `extraction_method`, resolves the instance, and **enforces consistency**: if `connector_type` is also supplied it must match the instance's registered `connector_type_id`, otherwise `KnowledgeError::Validation` is returned; when `connector_type` is omitted it is **derived** from the instance. An unregistered instance id is rejected (`ConnectorNotFound`-style validation). The `forget --source <slug>` filter now matches `connectors.slug` via subquery (plus the existing `source_types.name` arm), since the column is no longer a free-form string.



## Secret store (F10 / #187)

A single `SecretStore` trait backs every connector auth kind. One `SecretBundle` enum — `OAuth { access_token, refresh_token, expires_at }` | `ApiToken { token }` | `AppPassword { password }` — is keyed by the connector instance slug, so the supervisor, CLI, and server routes never branch on *which* store to talk to; they ask for a bundle by slug and pattern-match the kind.

```rust
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError>;
    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError>;
    async fn delete(&self, slug: &str) -> Result<(), SecretError>;
}
```

`FileSecretStore` is the V1 default: one JSON file per connector instance at `~/.local/share/mimir/secrets/<slug>.json`, internally-tagged (`{"kind":"oauth",…}`), **plaintext at rest**, file mode `0600`, parent directory `0700`. Loads **fail closed**: if the secret file or its directory has any group/other permission bits set, `load` returns `SecretError::InsecurePermissions` rather than reading and potentially leaking the credential. Stores and the directory-ensure step always (re)apply the tight modes, so a manually-loosened file/dir is re-tightened on the next `store`. Writes are atomic (temp file + `rename`) so a crash cannot leave a truncated secret that silently logs a connector out. `delete` is idempotent (a missing slug is `Ok`).

### Security model and deferrals

- **Plaintext at rest** is deliberate and consistent with the existing plaintext LLM API key in `config.toml` and the home-directory trust boundary. At-rest encryption (`argon2` + `chacha20poly1305`) is deferred (Phase 3 §7, out of scope) — see `VISION/09-Roadmap/Phase-3-Plan.md`. The earlier note in `VISION/03-Connectors/Technical-Design.md` saying tokens are "stored encrypted at rest" is **outdated**; the locked Phase-3 plan is the source of truth.
- **OS keyring** backend is tracked separately as #188 (deferred, feature-gated `secrets-keyring`, off by default because headless systemd boxes often lack a Secret Service daemon).
- **Path-traversal safety:** slugs are validated against `[A-Za-z0-9_-]{1,128}` before any filesystem access — empty, `..`, path separators, spaces, dots, and non-ASCII are rejected. The knowledge graph enforces slug uniqueness, but the store does not trust that.
- **Non-Unix targets:** file-mode enforcement is skipped (no portable mode concept). V1 targets Linux primarily; this is a documented limitation.

### `SecretBundle` design note

Struct variants (`ApiToken { token }`, `AppPassword { password }`) are used rather than newtype variants (`ApiToken(String)`) because serde's internally-tagged `kind` representation requires map-typed variant payloads; the named fields also make the on-disk JSON self-describing. `OAuth.refresh_token` and `OAuth.expires_at` are `Option` since not all grants issue a refresh token or return an expiry (e.g. client-credentials).

### In-memory backend

`InMemorySecretStore` (a `Mutex<HashMap<String, SecretBundle>>`) is included as a test/helper backend for the `mock` connector and unit tests; it is not for production persistence.

### End-to-end secret wipe

The `connector remove` flow (server `DELETE /connectors/:id` + CLI `mimir connector remove`, issues #202/#204/#203) calls `SecretStore::delete` on removal. F10 delivers the `delete(slug)` capability and its tests; the end-to-end "remove wipes the secret file" behaviour is verified when Epic 4 lands.


## What remains to be built

- **F11–F13** — optional OS-keyring backend (#188, deferred), rate limiter / retry primitives (done, #189), and the configurable mock harness (done, #190). The `SecretStore` + `FileSecretStore` + `InMemorySecretStore` landed in #187 / F10.
- **C1–C7** — the concrete backends (Photos, CalDAV Calendar, IMAP Email).
- **A1–A4** — server `AppState` registry/supervisor wiring + CLI subcommands. `mimir-server` declares the `mimir-connectors` dependency but does not yet own a `ConnectorSupervisor`; that wiring lands with A1 and depends on F8.

The framework pieces already landed: F1 (scaffold), F2 (instance table + facade), F3 (provenance FK), F4 (`normalize_and_insert` boundary), F5 (entity-resolution chain), F6 (the `Connector` trait + data types), F7 (the `ConnectorRegistry` + multi-backend factory dispatch), F8 (the `ConnectorSupervisor` supervised lifecycle), and F10 (the `SecretStore` + `FileSecretStore` + `InMemorySecretStore`).

## Verification

```bash
cargo build --workspace                              # full workspace
cargo build -p mimir-connectors --no-default-features # framework + mock only
cargo test -p mimir-connectors --test secrets_store  # secret store round-trip + perm + slug tests (F10)
cargo test -p mimir-connectors                        # includes supervisor lifecycle tests (F8)
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```
