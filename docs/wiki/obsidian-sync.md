# Obsidian Sync

> **Status:** Not yet implemented. Deferred to **post-Phase-5**.
> **Tracking:** Knowledge Graph roadmap — `kb import` / `kb export` (see `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` §2.16–2.17).

Mimir is designed to export and import its Knowledge Graph as a folder of Markdown files compatible with [Obsidian](https://obsidian.md), so you can browse, edit, and back up your knowledge in a tool you already use. This page describes the **planned design** so the intent and file format are documented ahead of implementation. None of the commands or behaviour below exists yet.

---

## Why Obsidian

Obsidian stores knowledge as plain Markdown files with YAML frontmatter and `[[wiki-links]]` — a portable, human-readable format that maps naturally onto Mimir's entity–fact graph:

- An **entity** becomes a `.md` file.
- A **fact** becomes a wiki-link relationship in that file.
- **Frontmatter** carries the machine-readable metadata (entity id, type, aliases) that lets Mimir re-import edits without losing provenance.

This keeps your data in a format you own and can read without Mimir, while still round-tripping through the Knowledge Graph.

---

## Planned Export (`kb export`)

```bash
mimir kb export ~/AgentKnowledge              # export to a folder
mimir kb export --format obsidian             # explicit format (default)
mimir kb export --format json | csv           # alternative machine formats
```

Export would write one `.md` file per entity to the target folder:

```markdown
---
entity_id: 7
name: Alice
type: Person
aliases: ["Al", "Alice Smith"]
---

# Alice

## Relationships
- [[works_at]] → [[Acme Corp]]
- [[lives_in]] → [[London]]
- [[visited]] → [[Rome]] (2025-05-03 to 2025-05-07)

## Sources
- calendar_event: Trip to Rome
- email: Rate your Roman History Tour
```

The `entity_id` in the YAML frontmatter links the file back to its Knowledge Graph row, so a later re-import can upsert rather than duplicate. Facts are rendered as wiki-links so Obsidian's graph view mirrors Mimir's.

---

## Planned Import (`kb import`)

```bash
mimir kb import ~/Obsidian/Vault/Personal --format obsidian
mimir kb import facts.csv --format csv --delimiter ,
mimir kb import facts.json --format json
mimir kb import facts.txt --format plaintext
```

Import would parse each file, resolve wiki-links back to entities (creating new entities where needed), and upsert facts with `source_type = Import` and a confidence of `0.80` (see [Confidence Model](Confidence-Model.md)). Conflict detection would flag facts that disagree with existing ones, and a `--dry-run` flag would preview the changes without writing.

Imported facts follow the same deterministic Rust pipeline as chat-extracted facts: validation, entity resolution, sensitivity gating, and insertion are all handled in Rust — the import source only supplies structured data.

---

## Planned File Format

| Element | Purpose |
|---|---|
| YAML frontmatter `entity_id` | Stable link back to the Knowledge Graph row for re-import |
| YAML frontmatter `type` | Entity type (`Person`, `Place`, `Event`, …) |
| YAML frontmatter `aliases` | Alternative names for search / dedup |
| `## Relationships` section | Facts as `[[predicate]] → [[object]]` wiki-links, with optional temporal bounds |
| `## Sources` section | Provenance for each fact |

Sensitive facts (e.g. allergies) would not be exported unless explicitly requested, and pending-confirmation facts would be excluded entirely.

---

## Limitations (Planned)

- **No bidirectional sync in the first release.** Export and import are one-way operations. A file watcher for live two-way sync (editing in Obsidian updates the KG, and vice versa) is a later enhancement.
- **No connector round-trip.** Export is a snapshot of current KG state; it does not re-fetch raw connector data.
- **Large graphs** may produce many small files; the export will be folder-organised by entity type.

---

## When It Will Arrive

This feature is deferred to **post-Phase-5** (after the Reasoning Engine and Proactive Agent phases). The Knowledge Graph, memory, inference, and nightly optimization that the export/import would surface are all implemented today — see [Knowledge Graph](knowledge-graph.md) — but the Obsidian bridge itself has not been built. Watch the roadmap for a tracking issue when work begins.
