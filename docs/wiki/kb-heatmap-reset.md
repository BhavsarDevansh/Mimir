# KB Heatmap and Reset

> **Issue:** #69

## What it does

Two knowledge-graph commands that make the graph's contents visible and its full wipe safe:

- `mimir kb heatmap` draws a picture of what Mimir knows — how many facts and entities it holds, which people or topics have the most facts, which types of fact are most common, how knowledge is spread over time, and how confident the graph is overall.
- `mimir kb reset` wipes the whole knowledge graph after a careful confirmation flow, so you cannot nuke years of memories by accident.

## Why it exists

The knowledge graph grows invisibly as Mimir learns. The heatmap turns it back into something you can glance at: "most of my facts are about Devansh and travel, and they cluster around last summer". The reset command exists because `kb forget --all` could already wipe everything, but doing so deserved a dedicated, hard-to-miss confirmation instead of a flag buried in a long command.

## How it works

### Heatmap

Run it any time; it reads live data from the daemon:

```bash
mimir kb heatmap
```

The output shows:

- **Totals** — facts, entities, average confidence.
- **Top entities by facts** — who/what the graph knows most about.
- **Predicates by facts** — the most common fact types (e.g. `located_in`, `works_as`).
- **Facts per month** — bucketed by the fact's `valid_from` date when set, otherwise its creation date.
- **Confidence distribution** — how much of the graph sits in each numeric confidence band: `explicit (1.0)`, `connector (0.7-1.0)`, `inference (0.4-0.7)`, `casual (<0.4)`.

Trashed facts are not counted, so the picture is of the live graph. For scripting or building your own visualisations:

```bash
mimir kb heatmap --json
```

### Reset

```bash
mimir kb reset
```

Mimir warns you with the live entity and fact counts, then asks you to type `DELETE EVERYTHING` exactly. After a 5-second countdown it backs up the knowledge database and deletes all facts, entities, preferences, and audit records. Connectors, configuration, and system settings are untouched.

The backup is printed on success and stored under `~/.local/share/mimir/backups/`. There is no undo from the trash for this command — the backup is the only recovery channel. If a script needs a non-interactive wipe, use `mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"` instead.

## Best practices

- Run `mimir kb heatmap` after a connector sync to see the graph grow.
- Use `--json` when you want to render the data yourself.
- Prefer `mimir kb forget` (soft delete, 30-day trash) for normal cleanup; reserve `mimir kb reset` for a fresh start.
- After a reset, the daemon keeps running; it simply has an empty graph and will re-learn from new chats and connector syncs.

## Related resources

- Technical details: `docs/kb-heatmap-reset.md`
- Full command reference: `docs/wiki/cli-commands.md`
