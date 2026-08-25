# Obsidian Export & Import

> **Issue:** #62

## What it does

Two commands that turn the knowledge graph into readable Markdown files and read edited Markdown files back into the graph: `mimir kb export` writes one `.md` file per entity (YAML frontmatter, wiki-links, and tidy sections for dates, relationships, preferences, and facts) to your export folder, and `mimir kb import <folder>` parses an Obsidian vault — exported by Mimir or hand-written — and adds everything to the knowledge graph.

## Why it exists

The knowledge graph is a database, and databases are hard to browse and edit by hand. Markdown is easy to browse, search, annotate, and edit in Obsidian (or any text editor). Export gives you a human-readable, portable view of what Mimir knows; import lets you correct, extend, or paste knowledge in bulk and have it flow back in with the same guarantees as any other fact (canonicalisation, corroboration, sensitivity checks, events).

## How it works

### Export

```bash
mimir kb export
```

This renders the whole graph into `~/AgentKnowledge/` (one file per entity, e.g. `Devansh.md`) and prints a summary of what was written. Useful variations:

- `mimir kb export --dir ~/MyVault` — pick a destination (also configurable permanently via `knowledge.export_dir` in `config.toml`, or the `MIMIR_KNOWLEDGE_EXPORT_DIR` environment variable).
- `mimir kb export --stdout` — print the files to the terminal instead of writing them (handy for piping elsewhere).
- `mimir kb export --json` — dump the raw export response for scripts.

### Import

```bash
mimir kb import ~/MyVault
```

The daemon scans the folder for `.md` files (recursively), parses each one, and adds the facts and preferences to the knowledge graph. Imported facts are marked as coming from the import and start at 0.80 confidence unless the file says otherwise. Things to know:

- **Idempotent:** importing the same files twice skips facts that already exist, and unchanged preferences are left alone too — nothing is duplicated. Preferences only change when the graph's existing preference is inferred with lower confidence than the import's 0.80; when a user-set or equal/higher-confidence preference keeps its value, the changed vault value is reported as a conflict instead of being silently skipped.
- **Try first:** `mimir kb import ~/MyVault --dry-run` reports exactly what would change (new/updated entities, new/existing facts, preferences, dates, errors) and writes nothing. Run without `--dry-run` to apply.
- **Editing entities:** change the `type`, `aliases`, or the `# Name` heading in a file and re-import to rename/retype the entity and sync aliases. Only explicit values count: a note without a frontmatter `type` or without a `#` heading never renames or retypes an existing entity.
- **Dates and events:** facts with a date and a recurrence (e.g. `- birthday → 1995-08-20 (1995-08-20, Birthday, yearly)`) recreate the events overlay, so they show up in upcoming events as usual.
- **Sensitive content:** anything the graph treats as sensitive (e.g. health facts) still lands in the confirmation queue instead of being applied silently.

## The file format at a glance

```markdown
---
type: Person
aliases: ["Dev"]
---

# Devansh

## Dates
- birthday → 1995-05-20 (1995-05-20, Birthday, yearly)

## Relationships
- married_to → [[Alice]] (since 2022-01-01)

## Preferences
- FoodPreference: favourite = Italian

## Facts
- allergic_to → peanuts (confidence: 1.0)
```

Sections are optional — a file can be just a heading, or a heading plus facts. Fact lines are `- predicate → object`; other entities are wiki-links `[[Name]]`; the parenthesised part holds dates, confidence, recurrence, and event type. Relationships also accept `- [[Alice]] — married_to` if you prefer reading the subject first.

## Best practices

- Keep one entity per file and keep the `# Heading` matching the entity name; the heading wins over the file name on import.
- Use `--dry-run` before a big import, especially when importing a folder you did not export yourself.
- Export regularly to a human-readable working copy; the vault mirrors your graph but is not a complete backup — global preferences stay outside the exported vault and are not restored by import.
- Don't move or edit the `entity_id` line in the frontmatter unless you mean to link to a specific existing entity — without it, import matches by name instead.

## Limitations

- Export is one-way (graph → files). Mimir does not watch the folder for changes yet; run `mimir kb import` after editing. Bidirectional sync is planned for a later phase.
- The `Dates` section covers facts with event overlays (birthdays, appointments, deadlines, tasks, reminders); plain old dates without events stay in `Facts`.
- Only entity-scoped preferences are exported; global preferences remain outside the exported vault and are not restored by import (v1 limitation).
