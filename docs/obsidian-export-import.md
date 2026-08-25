# Obsidian Export & Import

> **Scope:** `mimir kb export` and `mimir kb import` — Obsidian-compatible Markdown exchange with the knowledge graph.
>
> **Issue:** #62
> **Phase:** 2 (manual export/import; bidirectional file-watcher sync is deferred to Phase 3)

## Overview

The knowledge graph renders as a folder of Obsidian-compatible Markdown documents — one `.md` file per entity — with YAML frontmatter, wiki-links (`[[Name]]`), and a four-section grammar (`Dates`, `Relationships`, `Preferences`, `Facts`). Export is a one-way render from the graph to files; import parses a vault directory back into the graph through the shared `normalize_and_insert` pipeline, so imported facts inherit canonicalisation, corroboration/supersession/inference, the sensitivity gate, and the events overlay.

## Canonical format (v1)

```markdown
---
entity_id: 42
type: Person
aliases: ["Dev", "Devansh Bhavsar"]
created: 2025-06-15
updated: 2026-05-30
---

# Devansh

## Dates
- birthday → 1995-08-20 (1995-08-20, Birthday, yearly)

## Relationships
- married_to → [[Alice]] (since 2022-01-01)

## Preferences
- FoodPreference: favourite = Italian

## Facts
- allergic_to → peanuts (confidence: 1.0)
```

Section split: facts with an event overlay render into `Dates`; entity-object facts render into `Relationships` (objects as wiki-links); entity-scoped preferences into `Preferences`; literal-object facts into `Facts`. Event facts are excluded from the other sections. Fact-line attributes are comma-separated inside parentheses: `confidence: N.N` (override, default 0.80 on import), `since {date}` / `{date} to {date}` / `{date} to present` / `until {date}` (temporal bounds), a bare `{date}` (valid_from), a recurrence word (`yearly|monthly|weekly|daily`), and an event type (`Birthday|Appointment|Deadline|Task|Reminder|Custom`, Dates section only). Dates accept `YYYY-MM-DD`, RFC 3339, `YYYY-MM`, bare `YYYY` (1 January), and month-name forms (`Sep 2023`). The Relationships section additionally accepts the hand-written `- [[{object}]] — {predicate}` form. Preference lines are `- {Category}: {key} = {value}` with the `PreferenceCategory` wire names (`CalendarBehavior`, `NotificationStyle`, `FoodPreference`, `TravelPreference`, `WorkStyle`, `CommunicationPreference`, `General`).

The grammar lives in `mimir-knowledge/src/obsidian/grammar.rs` and is shared by render and parse, so the two directions cannot drift: `render_fact_line`/`render_preference_line` produce exactly the forms `parse_fact_line`/`parse_preference_line` accept, and the enum wire names (`EventType::as_str`/`FromStr`, `RecurrenceType::as_str`/`FromStr`, `PreferenceCategory::as_str`/`FromStr`/`TryFrom<i16>`) back both sides.

## Architecture

- `mimir-knowledge/src/obsidian/render.rs` snapshots the graph (`KnowledgeGraph::export_obsidian` → `render_all`): one document per entity, entities ordered by name, collision-suffixed stems for sanitised names that collide, frontmatter with `entity_id`/`type`/`aliases`/`created`/`updated`, and the four sections. Fact confidence is rendered explicitly; `NULL` `valid_until` renders as `to present`.
- `mimir-knowledge/src/obsidian/import.rs` parses and plans (`import_all` → `import_document`): YAML frontmatter is split with `mimir_core::frontmatter` and parsed with `serde_yaml` (already a workspace dependency); the entity is resolved by `entity_id` (the link back to the KG) or through the canonical resolution chain (exact name → alias → FTS5 fuzzy ≥ 0.9 → create, type-filtered; issue #182) via `resolve_or_create`/`pick_resolution`; name/type/alias changes on an existing entity are applied; facts go through `normalize_and_insert` with `source_type=Import` and `extraction_method=StructuredParse`, so imports default to confidence 0.80 unless the file carries a `confidence: N` attribute (which overrides, clamped to `[0, 1]`), and event facts recreate the events overlay from the parsed recurrence + event type + trigger date.
- Exact triples already present (same subject + predicate + object) are counted as existing and skipped, so re-importing an untouched export is idempotent. Preferences follow the engine's conflict rules: an import (0.80 confidence) overwrites an existing preference only when the stored one is inferred with lower confidence; user-set preferences and equal/higher-confidence inferred preferences win and are skipped without audit-log churn.
- Dry-run mode plans everything (entity resolution, existence checks) but never writes; the reported counts are exactly what an apply would change.
- Sensitivity gating is unchanged: a sensitive import lands in `pending_confirmation` for the normal confirm/reject flow.
- Global preferences (`entity_id IS NULL`) are a documented v1 limitation: export renders entity-scoped preferences only.

## Wire protocol

- `GET /kb/export` (bearer auth, read-only, no loopback gate) returns `ExportResponse` — `files` (ordered by relative path), `entity_count`, `fact_count`, `preference_count`, `event_count`. Handler: `mimir-server/src/routes/kb/export.rs`.
- `POST /kb/import` (bearer auth, loopback-gated like other privileged mutations) accepts `ImportRequest { path, dry_run }`, scans the directory recursively (hidden entries skipped, `.md` only, deterministic order), and returns `ImportResponse` with planned/applied counts and per-file errors. Handler: `mimir-server/src/routes/kb/import.rs`. A non-directory path is a 404.
- Client methods: `MimirClient::kb_export` / `MimirClient::kb_import` (`mimir-client/src/kb/obsidian.rs`).

## CLI

```bash
mimir kb export [--dir <path>] [--stdout] [--json]
mimir kb import <path> [--dry-run] [--json]
```

Export writes the bundle to `--dir`, else `knowledge.export_dir`, else `~/AgentKnowledge` (creating the directory), and prints a summary of entities/facts/preferences/dates written. `--stdout` prints the files concatenated with `<!-- mimir: {name} -->` separators; `--json` dumps the raw `ExportResponse`. Import prints a summary in both modes:

```text
Would import from ~/AgentKnowledge/:
  Entities: 12 new, 3 updated
  Facts: 47 new, 5 existing (skipped), 2 errors
  Preferences: 3 new, 1 updated
  Dates: 2 new
Run without --dry-run to apply.
```

The `[knowledge] export_dir` config key (`MIMIR_KNOWLEDGE_EXPORT_DIR` env override) sets the default destination; `~` is expanded.

## Testing

- `mimir-knowledge/tests/obsidian_test.rs` — format rendering, sanitisation, import planning/apply, dry-run, entity-id re-import, sensitivity, round trip.
- `mimir-server/tests/kb_obsidian_tests.rs` — route tests for export and import (dry-run, apply, re-import skip, missing directory).
- `mimir/src/kb/tests.rs` — CLI handler tests against a wiremock daemon (export writes files; import dry-run round trip).
