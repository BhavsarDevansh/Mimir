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

The temporal bounds come from the fact's `valid_from` / `valid_until`, so a
move is just a fact with bounds plus an overlay — no location-specific date
parsing. Both the conversational `remember` path (`extract.rs`) and connectors
(`MockFactConfig.location`, and future connector extraction) fill the same
field, differing only in provenance. `NormalizedFact` is `PartialEq`-only
(not `Eq`) because `f64` coordinates are not `Eq`.

## Pipeline: `apply_location_overlay`

In `normalize::process_normalized_fact`, after a non-sensitive fact is
inserted, if `location` is `Some`:

1. **Fill the missing half** via the injected `Geocoder`
   (`KnowledgeGraph::geocoder`, `Option<Arc<dyn Geocoder>>`):
   - address-only → `forward(address)` → lat/lng;
   - coords-only → `reverse(lat, lng)` → place name stored as `address`;
   - both present → stored as-is (no geocode);
   - neither → no-op.
   Geocoder `Err` and `Ok(None)` are logged and tolerated — the location is
   stored with whatever data it carries and the pipeline never aborts on a
   geocode failure. With no geocoder injected, the missing half stays empty.
2. **Upsert** via `KnowledgeGraph::upsert_location`: close any still-open
   location of the same `entity_id` + `location_type` whose `valid_from` is
   before the new `valid_from` (set its `valid_until = new.valid_from`), then
   insert the new row with `source_fact_id = fact_id`. Atomic in one
   transaction.

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

## Tests

`mimir-knowledge/tests/entity_locations_test.rs` covers: address-only forward
geocode; coords-only reverse geocode; both-present no-geocode; no-geocoder
address-only; geocoder-error tolerance; move supersession; connector-provenance
overlay; and the facade upsert directly.
