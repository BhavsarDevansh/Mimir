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
  location fact — done as of v0.81.0 (#196). The place name is the city/town
  (locality), so photos at different spots in the same city corroborate into
  one fact. The specific restaurant/landmark isn't stored as an entity yet —
  that level of detail is reserved for a future on-demand photo search.
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
component. The Photos connector (C2 / #196, v0.81.0) and the entity-locations
write path (S3 / #193) are wired in; the conversational location-search tool
(#98) lands in a subsequent Phase 3 issue.
