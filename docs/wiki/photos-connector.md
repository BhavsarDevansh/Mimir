# Photos Connector

> **Phase:** 3 — Connectors
> **Status:** Done (library) — issue #195 / C1. Daemon wiring and the
> `mimir connector …` CLI come in later Phase 3 issues; GPS → place naming is
> #196 (C2).

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
- Each photo becomes a `took_photo` fact for you (the owner), with the photo's
  timestamp and — if the photo has GPS — a location pin. Locations show up as
  "you were here around this time."
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
- The owner name is the subject of every fact. Set `owner_name` to how you want
  to be referred to in your knowledge graph (it defaults to the connector's
  slug).
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
default extensions cover JPEG, TIFF, HEIF/HEIC, PNG, and WebP.

## What's next (C2 / #196)

C1 persists the raw GPS coordinates as a location pin. C2 will
**reverse-geocode** those coordinates into a human place name ("Paris", "10
Downing St") so locations are searchable by name, not just by coordinates.
