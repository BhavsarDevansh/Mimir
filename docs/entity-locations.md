# Entity Locations — Write Path (Phase 3 S3 / #193)

> **Status:** Implemented (v0.78.0). Supersedes the write-path half of #65.
> **Depends on:** Geocoder service (#191), `normalize_and_insert` DRY boundary (#181),
> connectors table (#179). Proximity queries (`find_nearby`) are a separate issue (#196).

## Purpose

`entity_locations` stores structured geographic data (address, lat/lng, timezone)
for an entity with temporal validity windows, so Mimir can model moves
("home 2020–2023, home 2023–present") and later answer "where" questions /
proximity queries. Phase 2 created the table as a stub; this change implements
the write path and wires it into the shared extraction pipeline.

## Data model

The `entity_locations` table (migration `004`) gains a nullable
`source_fact_id INTEGER REFERENCES facts(id) ON DELETE SET NULL` (migration
`044`) that links a row to the fact that produced it — the same overlay-
provenance pattern as `events.fact_id`. A directly-seeded location (no
originating fact) leaves it `NULL`; forgetting the source fact keeps the
location (the FK becomes `NULL`).

Columns: `id`, `entity_id`, `location_type_id`, `address`, `latitude`,
`longitude`, `timezone`, `valid_from`, `valid_until`, `source_fact_id`,
`created_at`.

`location_types` (migration `001`) is `Home(1)`, `Work(2)`, `Visited(3)`,
`Origin(4)`, `Current(5)`, mirrored by `models::enums::LocationType`. (The
issue text that named `Previous/Frequent/EventLocation` predates Phase 2 and
is stale; the actual enum is the source of truth.)

> **No `confidence` column.** Locations do not carry their own confidence score
> in V1; provenance/traceability is via `source_fact_id` and the source fact's
> confidence. Adding confidence would require a management/update story that is
> deferred.

## Overlay on `NormalizedFact`

Following the events-subsystem pattern (a typed overlay deterministically
derived inside `normalize_and_insert`), a "where" fact carries an optional
`NormalizedLocation`:

```rust
pub struct NormalizedLocation {
    pub location_type: LocationType,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timezone: Option<String>,
}
```

The temporal bounds come from the **inserted fact's** `valid_from` /
`valid_until` (not the pre-correction extracted bounds), so a move is just a
fact with bounds plus an overlay — no location-specific date parsing. Reading
the bounds from the inserted `Fact` matters for corrections: `handle_correction`
mutates `new_fact.valid_from` before the insert (a `None` scope → `now`, a
datetime scope → that datetime), and the overlay must inherit the mutated
bound so the `entity_locations` row matches its source fact and prior-location
supersession fires. Both the conversational `remember` path (`extract.rs`) and
connectors (`MockFactConfig.location`, and future connector extraction) fill
the same field, differing only in provenance. `NormalizedFact` is
`PartialEq`-only (not `Eq`) because `f64` coordinates are not `Eq`.

## Pipeline: `apply_location_overlay`

In `normalize::process_normalized_fact`, after a non-sensitive fact is
inserted, if `location` is `Some` the work is **enqueued to a background
worker** rather than awaited inline, so a connector batch of location facts is
not gated on the geocoder's rate limit (~1 req/sec for Nominatim). The worker
processes jobs strictly in FIFO submission order, which preserves move /
supersession semantics within a batch and across separate
`normalize_and_insert` calls; a single worker loses no geocode throughput
versus parallelism with the default Nominatim backend, which is already
rate-limited to ~1 req/sec; a custom or self-hosted `Geocoder` with higher
throughput could make the single FIFO worker a bottleneck. Each job carries a
clone of the geocoder read at submit time and
the inserted fact's temporal bounds. Per job:

1. **Fill the missing half** via the job's `Geocoder`
   (`KnowledgeGraph::geocoder`, `Option<Arc<dyn Geocoder>>`):
   - address-only → `forward(address)` → lat/lng;
   - coords-only → `reverse(lat, lng)` → place name stored as `address`;
   - both present → stored as-is (no geocode);
   - neither → no-op.
   Geocoder `Err` and `Ok(None)` are logged and tolerated — the location is
   stored with whatever data it carries and the pipeline never aborts on a
   geocode failure. With no geocoder injected, the missing half stays empty.
2. **Upsert** via `queries::entity::upsert_location` (shared by the
   `KnowledgeGraph::upsert_location` facade): close any still-open location of
   the same `entity_id` + `location_type` whose `valid_from` is before the new
   `valid_from` (set its `valid_until = new.valid_from`), then insert the new
   row with `source_fact_id = fact_id`. Atomic in one transaction.

`KnowledgeGraph::flush_location_overlays` is a barrier that awaits every
overlay enqueued before the call, for deterministic graceful shutdown / tests.
Jobs enqueued concurrently with a flush are not guaranteed to have completed.
`AppState::shutdown` calls it after stopping the background scheduler, so
queued `entity_locations` upserts complete before resources are torn down.

The `Geocoder` trait lives in `mimir-core` (so `mimir-knowledge` can name it
without depending on `mimir-connectors`); the Nominatim default backend lives
in `mimir-connectors` and is injected by the server at startup
(`AppState::from_config_with_llm`).

## Pending (sensitive) path — deferred

Sensitive location facts land as `pending_confirmation` like any sensitive
fact, and the overlay is **not** applied until the fact is confirmed. Wiring
the overlay into `confirm_fact` is tracked as follow-up work (see Issues).

## Facade API

- `KnowledgeGraph::upsert_location(entity_id, location_type, address, lat, lng,
  timezone, valid_from, valid_until, source_fact_id)` — move/supersession
  semantics; the recommended write path.
- `KnowledgeGraph::insert_location(...)` — direct seed, no supersession.
- `KnowledgeGraph::get_locations(entity_id)` — list an entity's locations.
- `KnowledgeGraph::update_location(id, address, lat, lng, timezone)` — partial
  mutable-field update.
- `KnowledgeGraph::set_geocoder(Arc<dyn Geocoder>)` / `geocoder()` — inject /
  read the geocoder backend.
- `KnowledgeGraph::flush_location_overlays()` — await every overlay enqueued
  before the call (deterministic shutdown / tests).

## Tests

`mimir-knowledge/tests/entity_locations_test.rs` covers: address-only forward
geocode; coords-only reverse geocode; both-present no-geocode; no-geocoder
address-only; geocoder-error tolerance; move supersession; connector-provenance
overlay; a batch of location facts persisted after a flush; correction overlays
(no-scope and datetime-scope) using the inserted fact's bounds to supersede a
prior open location; and the facade upsert directly.
