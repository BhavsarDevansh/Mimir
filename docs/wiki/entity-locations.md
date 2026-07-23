# Entity Locations

> **Since:** v0.78.0 (Phase 3 S3 / #193)

## What it is

Mimir can remember **where** an entity is or was — your home, workplace, a
place you visited, your origin, or your current location — as structured
address + GPS data with a time window. This lets it model moves over time
("home 2020–2023, home 2023–present") and lays the groundwork for
location-aware features such as "what's near my home" (proximity queries land
in a later release).

## How it works

When you tell Mimir something like *"I live at 10 Downing St"* (or a connector
extracts an address/GPS fix), the fact carries a small structured **location
overlay** alongside the usual subject–relationship–object triple. Mimir:

1. **Geocodes the missing half** — if you give only an address it looks up the
   coordinates; if you give only coordinates it looks up the place name. It
   uses the built-in OSM Nominatim geocoder (free, no API key). If geocoding
   fails or finds nothing, the fact is still stored with whatever you gave.
2. **Records the location** for the entity with the fact's time bounds.
3. **Handles moves** — adding a new home with a start date automatically closes
   the previous open-ended home at that date, so the history stays consistent.

The location row links back to the fact that produced it, so it's traceable and
honours forgetting (forgetting the fact keeps the address but unlinks it).

## Location types

`Home`, `Work`, `Visited`, `Origin`, `Current`.

## Use cases

- Remembering home / work addresses and past moves.
- Capturing visited places (from photos GPS, calendar event locations, shipping
  addresses) once connectors come online.
- Future proximity queries ("find places near my home").

## Notes / limits (v0.78.0)

- Locations don't carry their own confidence score yet — provenance is via the
  source fact.
- A *sensitive* "where" fact is held for confirmation like any sensitive fact;
  its location is applied once you confirm it (follow-up work).
- Geocoder settings (self-hosted Nominatim endpoint, disable toggle) are not yet
  configurable — the public-instance defaults are used.
