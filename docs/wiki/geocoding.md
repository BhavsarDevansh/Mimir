# Geocoding

> **New in v0.77.0** (Phase 3, Issue #191).

## What it is

Geocoding turns a place name or address into map coordinates (forward) and
turns coordinates back into a place name (reverse). Mimir uses it so the
knowledge graph can attach real locations to facts — e.g. a photo's GPS becomes
a named place, or an address in an email becomes coordinates you can search
nearby.

## How it works

Mimir talks to the free **OpenStreetMap Nominatim** service by default — no API
key, no account. To respect Nominatim's free-service rules, Mimir sends at most
**one request per second**, identifies itself with a descriptive `User-Agent`,
and retries automatically (with backoff) if the service is briefly unavailable.
If Nominatim genuinely has no result, Mimir records "no match" rather than
guessing; if the network fails, it logs the error and continues — it never
crashes on a bad address.

The geocoder is pluggable: Nominatim is the default backend, but the design
allows other providers (e.g. Mapbox) to be added later. You can also point
Mimir at a self-hosted Nominatim instance for heavier use.

## Use cases

- **Photos:** a photo's GPS coordinates resolve to a place name and become a
  location fact (coming with the Photos connector).
- **Entity locations:** an address extracted from email/calendar is geocoded to
  coordinates so proximity queries ("find places near X") work.
- **Location search tool:** ask Mimir "where is London?" and get candidates
  with coordinates, country, and alternative names (planned, #98).

## Best practices

- Heavy or repeated bulk geocoding should run against a **self-hosted
  Nominatim** instance rather than the shared public one.
- Set a contact email in configuration when using the public instance; Nominatim
  recommends it and it helps if your usage is ever flagged.
- Geocoding is best-effort: an address that cannot be resolved is stored as the
  raw address (no coordinates) rather than blocking ingestion.

## Status

The geocoder abstraction and Nominatim backend are in place as a library
component. Wiring it into the Photos connector, the entity-locations write
path, and the conversational location-search tool lands in subsequent Phase 3
issues.
