# Entity Locations — Write Path (Phase 3 S3 / #193)

> **Status:** Implemented. Write path in v0.78.0 (#193, write half of #65);
> proximity query in v0.79.0 (#194, query half of #65 — closes #65); place
> coordinate anchoring (`Geographic` type) in v0.81.0 (#196, Phase 3 C2).
> **Depends on:** Geocoder service (#191), `normalize_and_insert` DRY boundary (#181),
> connectors table (#179). Proximity queries (`find_nearby`) are implemented
> in v0.79.0 (Phase 3 S4 / #194).

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
`Origin(4)`, `Current(5)`, `Geographic(6)` (added in migration `046` for
Phase 3 C2 / #196), mirrored by `models::enums::LocationType`.

`Geographic` is distinct from the person-location types above: a `Place`
entity does not "visit" a location, it *is* one. The Photos connector (C2)
anchors each place entity created from a photo's GPS with a coordinate row
typed `Geographic`, so `find_nearby` can resolve places by coordinates rather
than only by where the owner has been. A place's coordinates are timeless — a
place does not move — so the `Geographic` row uses the idempotent
`ensure_place_coordinates` instead of `upsert_location`'s move/supersession
semantics; repeated photos at the same place must not pile up closed
move-history rows. The single-row-per-place invariant is enforced at the schema
level by a partial unique index on `entity_id` scoped to
`location_type_id = 6` (`idx_entity_locations_geographic_unique`, migration
`047`); `ensure_place_coordinates` is a single atomic
`INSERT ... ON CONFLICT DO UPDATE` against that index, so it is race-free even
if the overlay worker is later parallelised. The index is deliberately partial
— `Visited`/`Home`/`Work`/`Origin`/`Current` rows are not unique per
`(entity_id, location_type_id)` (a person has many `Visited` rows), so a full
unique index would break them.

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

### Write serialisation (issue #236)

Both the ingestion caller (`normalize_and_insert`) and the background overlay
worker write to the same SQLite database. In WAL mode a *deferred* transaction
that reads (entity resolution, temporal-overlap check), then has another
connection commit, then writes is rejected with an immediate, un-retriable
`SQLITE_BUSY` — the read snapshot is stale, so `busy_timeout` cannot wait it
out. This surfaced as flaky `entity_locations` tests (a location overlay
silently dropped, or an `insert_fact` failing) and could fail a real ingestion
under heavy connector load.

The fix is a shared `KnowledgeGraph::write_lock` (`tokio::sync::Mutex`):
`normalize_and_insert` holds it per-fact across its read-then-write
transaction, and the overlay worker holds it across the `upsert_location` +
`ensure_place_coordinates` DB writes (but **not** the geocode network call,
which stays off-thread). The two writers can therefore never commit
concurrently, eliminating the stale-snapshot `SQLITE_BUSY`. Reads stay fully
concurrent (the lock is held only across write transactions).

The `Geocoder` trait lives in `mimir-core` (so `mimir-knowledge` can name it
without depending on `mimir-connectors`); the Nominatim default backend lives
in `mimir-connectors` and is injected by the server at startup
(`AppState::from_config_with_llm`).

## Pending (sensitive) path (issue #226)

Sensitive "where" facts land as `pending_confirmation` like any sensitive fact, and the overlay is **not** applied while the fact is pending — no `entity_locations` row exists until the user confirms it. To keep the structured geo data across the confirmation boundary, the sensitive path in `normalize::process_normalized_fact` persists the `NormalizedLocation` shape into the `pending_location_meta` table (migration `048`, the location analogue of `pending_event_meta` for events): typed `location_type_id` FK plus `address` / `latitude` / `longitude` / `timezone`, keyed on the pending `fact_id` with `ON DELETE CASCADE`.

`extract::confirm_fact` then rebuilds the overlay exactly like the non-sensitive path: it reads the persisted shape, re-runs the same geocode-fill + `upsert_location` (`normalize::apply_location_overlay`, called directly rather than enqueued — confirmation is a single user-initiated action, not a batch), using the **confirmed fact's** id and temporal bounds, and consumes the meta row afterwards. Rejecting the pending fact hard-deletes the fact, so the `ON DELETE CASCADE` foreign key removes the meta row automatically — no orphan location row can be left behind. Legacy pending facts that predate the `pending_location_meta` table have no shape and get no overlay, matching the events-subsystem's legacy fallback behaviour. One deliberate asymmetry with the non-sensitive path: `place_anchor` (Phase 3 C2) is not rebuilt — `pending_location_meta` stores the `NormalizedLocation` shape only, so a sensitive Place-object fact gets the subject's location row but no `Geographic` anchor for the place.

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
- `KnowledgeGraph::find_nearby(lat, lon, radius_km, at)` — proximity query
  (Phase 3 S4 / #194); see [Proximity query](#proximity-query).
- `queries::entity::ensure_place_coordinates(place_id, lat, lon, source_fact_id)`
  — idempotent anchor for a `Place` entity's `Geographic` coordinates (Phase 3
  C2 / #196). Not (yet) on the `KnowledgeGraph` facade; called by the
  location-overlay worker.

## Tests

`mimir-knowledge/tests/entity_locations_test.rs` covers: address-only forward
geocode; coords-only reverse geocode; both-present no-geocode; no-geocoder
address-only; geocoder-error tolerance; move supersession; connector-provenance
overlay; a batch of location facts persisted after a flush; correction overlays
(no-scope and datetime-scope) using the inserted fact's bounds to supersede a
prior open location; and the facade upsert directly.

<a id="proximity-query"></a>
## Proximity query (Phase 3 S4 / #194)

`KnowledgeGraph::find_nearby(latitude, longitude, radius_km, at)` returns
every `entity_location` within `radius_km` of the query point, sorted
nearest-first, as `Vec<NearbyLocation>` — each entry carries the `EntityLocation`
row and its exact great-circle `distance_km`.

### Two-stage strategy

1. **Bounding-box pre-filter (SQL, approximate).** `geo::bounding_box` computes
   an over-inclusive lat/lon box around the query point (latitude span
   `radius / 111.32` deg per side; longitude divided by `cos(lat)`, clamped at
   the poles). SQLite scans `entity_locations` with
   `WHERE latitude IS NOT NULL AND longitude IS NOT NULL AND latitude BETWEEN
   ? AND ? AND longitude BETWEEN ? AND ?`, using the composite index
   `idx_entity_locations_coords(latitude, longitude)` (migration `045`). NULL
   coordinates (address-only locations) are skipped entirely.
2. **Haversine post-filter (Rust, exact).** For each candidate the exact
   great-circle distance `geo::haversine_km` is computed; points beyond
   `radius_km` (edge-of-box over-inclusions) are dropped and the survivors are
   sorted ascending by distance. Sorting in Rust (not SQL) keeps the distance
   computation single-source and avoids a redundant SQLite sort over the small
   survivor set.

### Temporal scoping

`at: Option<DateTime<Utc>>` optionally restricts to locations whose validity
window contains the instant: `valid_from IS NULL OR valid_from <= t` **and**
`valid_until IS NULL OR valid_until >= t`. `None` is a pure spatial query over
all locations (including historical `Visited`/`Origin` overlays); `Some(t)`
answers "where was X located at time t".

### Pure helpers

`geo::haversine_km` (great-circle distance) and `geo::bounding_box` live in
`mimir-knowledge::geo` — pure, `unsafe`-free, allocation-free, unit-tested and
benchmarked (`benches/pure_helpers.rs`). No external `geo` crate is used: the
Haversine formula is a few lines and a heavy dependency for one function would
violate the minimal-dependency stance. The spherical Earth model (mean radius
`6371.0088` km) is accurate to ~0.5%, ample for personal-scale proximity.

### Tests

`mimir-knowledge/tests/find_nearby_test.rs` covers: within-radius sorted
nearest-first; an edge-of-box point outside the radius excluded by the exact
post-filter; NULL-coordinate locations skipped; temporal scoping (open-ended
current home vs. closed previous home) with and without an `at` instant; and an
out-of-range query returning nothing. `mimir-knowledge/src/geo.rs` unit-tests
the Haversine (London–Paris, coincident, symmetric, antipodal) and bounding-box
(pole clamping, equator widening) helpers.
