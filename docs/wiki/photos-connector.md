# Photos Connector

> **Phase:** 3 — Connectors
> **Status:** Done (library + daemon/CLI) — C1 (#195) + C2 (#196). Daemon wiring (A1 / #202), action routes (A2 / #203), and the `mimir connector …` CLI (A3 / #204) have landed.

## What it is

The Photos connector reads your local photo library and turns each photo's
metadata into a fact in your knowledge graph: **where** you took it (from EXIF
GPS) and **when** (from the EXIF datetime). It watches a folder you choose and
learns about new photos automatically — no cloud, no upload, no OAuth. Your
photos stay on your device; only their metadata enters Mimir.

It is read-only. Mimir never modifies or deletes your photos.

## How it works

- It **watches** a directory (recursively) using your OS's file watcher
  (inotify on Linux, FSEvents on macOS, ReadDirectoryChanges on Windows).
- When a photo is added or changed, a short (~2s) debounce coalesces the
  burst of file events into one update.
- It reads the photo's **EXIF** metadata (the same data cameras and phones
  embed) using a pure-Rust parser that handles JPEG, TIFF, HEIF, PNG, and WebP.
- Each photo becomes a fact for you (the owner). When the photo has GPS,
  Mimir reverse-geocodes the coordinates into a place name (using the built-in
  geocoder) and records **"you took a photo at <place>"** — e.g. "you took a
  photo at Rome" — with the timestamp. The place becomes a searchable entity
  in your knowledge graph, and multiple photos at the same place corroborate
  into one stronger fact instead of cluttering the graph.
- When a photo has GPS but no place name can be resolved (no geocoder, no
  match, or a temporary geocoder outage), Mimir records **"you visited
  <coordinates>"** instead — e.g. "you visited 46.500, 7.500" — so the
  real-world visit is the fact and the photo itself stays as its provenance
  (the file path is never the fact's object). Photos taken at the same spot
  corroborate into one fact per spot.
- It remembers which files it has already processed, so unchanged photos are
  **never re-scanned** — even after you restart Mimir. New and modified photos
  are picked up; deleted photos are dropped from the record on the next full
  scan (a restart).

The first time it runs against an existing library it ingests every photo once
(to learn the history). After that it only reacts to changes.

## Use cases

- **"Where was I in May 2024?"** — Mimir can answer from your photo locations,
  not just your calendar.
- **"Show me everywhere I've taken photos."** — a map of visited places built
  passively from EXIF.
- **Trip reconstruction** — photos timestamp + location give Mimir a timeline
  of where you were, which it can combine with calendar events and emails.

## Best practices

- Point it at a stable folder (e.g. `~/Pictures` or a synced photo library).
  Avoid pointing it at a temp/download folder that churns constantly.
- The subject of every fact is your canonical identity — the `[identity] name`
  in your config — so photo facts line up with the same "you" the rest of
  Mimir uses (and surface in user-scoped memory sections). `owner_name` is
  only a fallback for when no identity is configured (it defaults to the
  connector's slug).
- Photos without GPS still record a `took_photo` fact with the timestamp —
  useful for "how many photos did I take last year?" — they just don't pin a
  location.
- RAW formats (`.cr2`, `.arw`, `.nef`) aren't parsed yet; export to JPEG/TIFF
  or wait for a follow-up. HEIC/HEIF, PNG, and WebP are supported.

## Configuration

When the daemon wiring lands, a Photos connector is configured with a JSON
object like:

```json
{
  "watch_dir": "/home/me/Pictures",
  "owner_name": "Devansh",
  "debounce_ms": 2000,
  "extensions": [".jpg", ".jpeg", ".heic"]
}
```

`watch_dir` is required and must exist; the other fields are optional. The
default extensions cover JPEG, TIFF, HEIF/HEIC, PNG, and WebP. `owner_name`
is only used when no `[identity] name` is configured — with an identity set,
photo facts are authored against it instead.

## Location enrichment (C2 / #196)

C2 is now implemented (#196): GPS coordinates are reverse-geocoded into a
place name, photos at the same place corroborate, and the place's coordinates
are anchored, so proximity queries ("places near this point") resolve places by
where they are, not just by where you've been. If a place can't be resolved
(no geocoder, no match, or a transient network error), the photo still records
a `visited <coords-label>` fact, with the photo path retained as provenance, so no data is lost.

### Photos as facts, not entities

A photo is stored as a **fact**, not a knowledge-graph entity. The only
entities created are you (the owner) and one `Place` per distinct locality
Mimir sees. The geocoder resolves the most specific populated locality field
available — `city`, `town`, `village`, `hamlet`, `municipality`, `county`,
`state`, or `region` — so a photo in a hamlet and a photo in a city both
anchor to the appropriate locality-level `Place`, not just city/town names.
So your knowledge graph grows with the *places you visit*, not with the number
of photos you take — thousands of photos across a handful of localities stay
a handful of place facts. Each photo's file path is kept as
provenance, which is the trail a future "find that photo" search will walk.
