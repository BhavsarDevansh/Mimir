# Entity Locations

> **Since:** v0.78.0 (Phase 3 S3 / #193); proximity query added v0.79.0
> (Phase 3 S4 / #194).

## What it is

Mimir can remember **where** an entity is or was — your home, workplace, a
place you visited, your origin, or your current location — as structured
address + GPS data with a time window. This lets it model moves over time
("home 2020–2023, home 2023–present") and lays the groundwork for
location-aware features such as "what's near my home" (proximity queries are
available in v0.79.0; see below).

## How it works

When you tell Mimir something like *"I live at 10 Downing St"* (or a connector
extracts an address/GPS fix), the fact carries a small structured **location
overlay** alongside the usual subject–relationship–object triple. Mimir:

1. **Geocodes the missing half** — if you give only an address it looks up the
   coordinates; if you give only coordinates it looks up the place name. It
   uses the built-in OSM Nominatim geocoder (free, no API key). If geocoding
   fails or finds nothing, the fact is still stored with whatever you gave.
2. **Records the location** for the entity with the inserted fact's time
   bounds (so a correction like *"actually I live at Y now"* correctly retimes
   and supersedes the prior home).
3. **Handles moves** — adding a new home with a start date automatically closes
   the previous open-ended home at that date, so the history stays consistent.
4. **Doesn't block on geocoding** — the address/GPS lookup + write happen on a
   background worker, so remembering many places at once (e.g. a connector
   importing lots of GPS-tagged photos) isn't slowed to the geocoder's ~1
   lookup/sec rate.

The location row links back to the fact that produced it, so it's traceable and
honours forgetting (forgetting the fact keeps the address but unlinks it).

## Location types

`Home`, `Work`, `Visited`, `Origin`, `Current`, and `Geographic`.

`Geographic` is special: it anchors a **place** entity's own coordinates
(e.g. the city "Rome" is at this lat/lon), rather than where a person is. The
Photos connector uses it so proximity queries can find places by where they
are, not just by where you've been. A place's coordinates are timeless — a
place doesn't move — so Mimir keeps a single `Geographic` row per place even
if you take many photos there.

## Use cases

- Remembering home / work addresses and past moves.
- Capturing visited places (from photo GPS data, calendar event locations,
  shipping addresses) once connectors come online.
- Proximity queries — "find places near my home" (v0.79.0, see below).
- **Place anchoring from photos** — when the Photos connector reverse-geocodes
  a photo's GPS to a place, that place's coordinates are anchored as a
  `Geographic` row (v0.81.0, #196), so "what places are near this point" works
  for the places themselves.

## Proximity queries (v0.79.0)

`find_nearby(latitude, longitude, radius_km, at)` returns every remembered location
within a given radius of a point, sorted by distance (nearest first). Each
result includes the exact distance in kilometres.

**How it works:** Mimir does a fast coarse filter in SQLite — draw a box
around the point and grab only the locations inside it (using a coordinate
index) — then computes the exact distance for each of those few candidates and
keeps only the ones truly within the radius. This is fast even with many
locations and always correct (the box is deliberately a little generous, the
exact distance is the final arbiter).

**Time scoping:** you can ask "where was this near, *as of* June 2024?" by
passing a date. Without one, all locations are searched, including past visits.

Locations stored without coordinates (an address Mimir couldn't geocode) are
skipped by proximity searches — they have no point to measure distance from.

## Notes / limits (v0.78.0)

- The background overlay worker uses an unbounded queue, so a very large burst
  of location facts (thousands) is held in memory while the geocoder catches up
  at ~1 lookup/sec; graceful shutdown calls `flush_location_overlays` to drain
  queued jobs before tearing down resources. The worker's database writes are
  serialised with the ingestion caller's writes via an internal lock so the two
  never commit at the same time — this avoids spurious "database is locked"
  failures when many locations are remembered at once.
- Locations don't carry their own confidence score yet — provenance is via the
  source fact.
- A *sensitive* "where" fact is held for confirmation like any sensitive fact;
  its location is applied once you confirm it (follow-up work).
- Geocoder settings (self-hosted Nominatim endpoint, disable toggle) are not yet
  configurable — the public-instance defaults are used.
