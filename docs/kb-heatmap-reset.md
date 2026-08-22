# KB Heatmap and Reset

> **Scope:** `mimir kb heatmap` and `mimir kb reset` — the deferred Phase 3+ CLI polish commands.
>
> **Issue:** #69

## `mimir kb heatmap`

`mimir kb heatmap [--json]` renders a knowledge-density snapshot of the knowledge graph as terminal bar charts. It is a pure read path: the CLI fetches one aggregate response from the daemon and renders it locally, so the terminal UI adds no daemon-side state.

### Wire protocol

`GET /kb/heatmap` (bearer-auth, read-only, no loopback gate) returns a `HeatmapResponse`:

| Field | Meaning |
|-------|---------|
| `facts` | Live (non-forgotten) fact count |
| `entities` | Entity count (all rows in `entities`) |
| `avg_confidence` | Mean confidence over live facts (`0.0` when empty) |
| `top_entities` | Top 10 entities by fact count, ties by name ascending |
| `predicates` | Top 10 predicates by fact count, ties by name ascending |
| `temporal` | Facts per `YYYY-MM` bucket of `valid_from` (falling back to `created_at`), ascending |
| `confidence_bands` | Facts per confidence band, always in fixed order: `explicit (1.0)`, `connector (0.7-1.0)`, `inference (0.4-0.7)`, `casual (<0.4)` |

The SQL lives in `mimir-knowledge/src/queries/heatmap.rs` behind the `KnowledgeGraph::heatmap()` facade delegate (`mimir-knowledge/src/graph/heatmap.rs`); the handler (`mimir-server/src/routes/kb/heatmap.rs`) maps the knowledge-level rows to the wire types (`mimir-api-types`). The client method is `MimirClient::kb_heatmap` (`mimir-client/src/kb/query.rs`).

### Semantics

- **Trash exclusion.** Forgotten (trashed) facts (`fact_status_id = 6`) are excluded from every fact count, distribution, and band, so the heatmap reflects the live graph, not the trash. The other statuses (Active, Inferred, Disputed, Corrected, Superseded) all count.
- **Temporal bucket.** Each fact lands in the month of its `valid_from` when set, otherwise the month of `created_at` — the earliest useful temporal signal for the fact.
- **Confidence bands.** Band membership is `confidence = 1.0` (explicit), `[0.7, 1.0)` (connector), `[0.4, 0.7)` (inference), `[0.0, 0.4)` (casual). The labels are the issue #69 display names, not an authoritative source-type mapping — a `UserEdit` fact below 1.0 still appears in its numeric band. Bands sum to `facts`.
- **Ranked lists.** Both ranked sections are top 10 by count, ties broken by name ascending, so output is deterministic.

### Rendering

`mimir/src/kb/heatmap.rs` renders the response with `█` bars scaled to the section maximum (20 cells), counts formatted with thousands separators, and a `(none)` placeholder per empty section. `--json` prints the raw `HeatmapResponse` instead. The earlier design discussion floated `ratatui` for an interactive TUI; the shipped version deliberately stays text-only (no new dependency, scriptable, consistent with the rest of the `kb` group) — the `--json` shape is stable for external renderers.

## `mimir kb reset`

`mimir kb reset` is a dedicated, safer full-wipe flow on top of the existing `kb forget --all` machinery. It requires an interactive terminal; with piped stdin it exits with a pointer to the non-interactive form (`mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"`).

### Flow

1. The CLI fetches live counts via `GET /kb/heatmap` and prints the warning (entity + fact totals, what survives, no-trash-recovery notice).
2. The user must type the exact phrase `DELETE EVERYTHING` (case-sensitive). Any other answer aborts with no request to the daemon.
3. A 5-second countdown runs in the CLI.
4. The CLI dispatches the existing `POST /kb/facts/forget` with `all: true` and `confirmation_phrase: "DELETE EVERYTHING"` — the daemon re-validates the phrase, creates a timestamped SQLite backup (`VACUUM INTO`) under `~/.local/share/mimir/backups/`, and hard-deletes facts, entities, preferences, queues, trash, and provenance rows via `forget_all` (`mimir-knowledge/src/forget/trash.rs`). The wipe request is logged to the daemon log by the shared HTTP trace layer.

### Safety properties

- The phrase check exists in both layers: the CLI aborts locally on mismatch, and the daemon independently rejects any forget-all request without the phrase, so a direct API caller gets the same safeguard.
- The backup is created before any deletion, and the CLI prints the backup path on success.
- Unlike `kb forget --all --archive`, the reset path hard-deletes (no trash recovery) — the pre-wipe backup is the only recovery channel.

### Recovery

There is no `kb restore --from-backup` command (the issue's example flow references one, but it is a separate feature; the current restore surface is trash-based `mimir kb restore --all`). To recover from a reset: stop the daemon, replace the knowledge database file with the backup printed by the reset (or under `~/.local/share/mimir/backups/`), and start the daemon again.

## Testing

- `mimir-knowledge/tests/heatmap_tests.rs` — totals, distributions, band boundaries, forgotten-fact exclusion, empty graph.
- `mimir-server/tests/kb_heatmap_tests.rs` — `GET /kb/heatmap` route over a seeded in-process daemon, plus the empty-graph shape.
- `mimir/src/kb/tests.rs` — bar rendering (scaling, thousands separators, zero bars) and the reset flow against a wiremock daemon: wrong phrase never reaches `POST /kb/facts/forget`, confirmed phrase dispatches the wipe and reports the backup.
