# Memory System

## Philosophy

Mimir's memory is its executive summary — a small, curated, always-hot block of the most critical facts, system context, and pointers to deeper knowledge. It is injected into every system prompt so that even the very first interaction is grounded in what Mimir already knows.

Think of it as the agent's **index card** — not the full encyclopedia, but the page numbers of the most important chapters.

Memory is a *view* over the Knowledge Graph, not a separate store. The Knowledge Graph holds every fact at full granularity with provenance and history; the memory block is the ranked subset that fits inside the system prompt's character budget. Facts that do not make the cut are not lost — they remain queryable via `mimir kb query`, `mimir kb show`, and `mimir kb browse`.

## Role in the Architecture

```
User Query
    ↓
System Prompt (personality preset + operating directives + condensed memory + upcoming section)
    ↓
LLM reasons about the query
    ↓
If the condensed memory is insufficient → `retrieve_context` tool → Knowledge Graph, Connectors, or Reasoning Engine
```

The condensed memory reduces the need to hit the Knowledge Graph for trivial lookups. The LLM already knows the user's name, their current city, their primary email, their favourite editor, their upcoming flight. This saves tokens, latency, and complexity.

## How It Works

The memory block is rendered on demand from the Knowledge Graph by deterministic Rust code in `mimir-knowledge/src/queries/memory/` and `mimir-knowledge/src/condensation.rs`:

1. Query the Knowledge Graph for facts about the user
2. Score each fact with the ranking formula and select the top facts within the character budget
3. Render the schema as deterministic text and send it to the LLM for natural-language condensation; a hash of the top-N facts (default 500, `condensation_top_n`) gates the cached result so unchanged memory skips the LLM call
4. Validate the LLM output against the budget in Rust; fall back to deterministic template rendering on LLM failure or oversize output
5. Cache the result in the `system_state` table (`key = "condensed_memory"`) so `mimir memory` prints instantly and daemon restarts survive

The legacy `memory.md` file-backed system was deleted in v0.37.0 (issue #111). There is no text file to keep in sync — the condensed block is always derived from the graph.

## Ranking Formula

Facts are scored deterministically in Rust:

```
score = confidence × category.memory_weight × temporal_boost × priority × centrality
```

| Factor | Meaning |
|--------|---------|
| Confidence | How certain Mimir is, based on source quality and corroboration |
| Category weight | Identity facts rank at the top, followed by preferences, relationships, and the rest of the taxonomy |
| Temporal boost | Facts with a future `valid_from` (upcoming events and tasks) score higher the closer the date is, so imminent items surface |
| Priority | `memory_priority_id` gives critical facts a 2× multiplier |
| Centrality | Facts about well-connected entities (people mentioned often) rank higher |

Identity facts are always rendered first. The fill algorithm sorts the remaining facts by score descending, greedily fills the character budget (default 2,500), and truncates the last entry with `…` when the budget is exceeded.

## Upcoming Section

Alongside the condensed core-facts block, the daemon renders an upcoming-events section from the events overlay (`render_upcoming_section`): future one-time events, recurring events, and tasks that require user action. It is injected into the system prompt together with the condensed memory so the LLM surfaces what is coming up without extra tool calls.

## Identity Fact Seeding

When the server starts, it resolves the user entity from `[identity].name` in config. If the entity exists (or is created), the server seeds two identity facts automatically: `has_name` (the full name) and `preferred_name` (only if it differs). Both are categorised as Identity (category ID 110) so they rank at the top of the memory schema. Seeding is idempotent, so Mimir knows the user's name immediately after initialization without requiring a separate conversation to extract it.

## Regeneration Triggers

Condensed memory is regenerated when:
- A fact is inserted/updated/deleted that ranks in the top-N for memory inclusion (demand-driven via `BackgroundScheduler`)
- `mimir memory --refresh` is called explicitly (force-submit)
- Nightly optimization completes (confidence recalculation may re-rank facts)

The scheduler ensures condensation only runs during LLM downtime so it never competes with active chats.

## Context Injection

The memory-bearing system prompt is composed at session creation for non-incognito sessions — after the preset tone text and shared operating directives — and reused for the session's lifetime, preserving the LLM's prefix cache; incognito requests build a fresh prompt per request. The block is framed as a curated subset rather than an exhaustive picture ("Core facts about the user (condensed subset — not a complete picture; treat as starting context, not exhaustive)"), signalling to the LLM that it should use the `retrieve_context` tool if it needs more.

The core block is frozen per session: non-incognito sessions reuse the system prompt captured at session creation; incognito requests build a fresh prompt. The one exception is the request-local temporal anchor at the start of the composed prompt: `Now: <RFC 3339 UTC> (<weekday> <date>)` is refreshed for every turn, while the condensed facts and upcoming section remain frozen.

## Configuration

```toml
[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
condensation_top_n = 500
```

## Implementation

- `mimir-knowledge/src/queries/memory/` — ranking, scoring, budget fill, rendering
- `mimir-knowledge/src/models/memory.rs` — `MemorySchema`, `MemoryBucket`, `RankedFact`
- `mimir-knowledge/src/condensation.rs` — LLM condensation pipeline with deterministic fallback
- `mimir-knowledge/src/queries/system_state.rs` — `condensed_memory` cache read/write
- `mimir-server/src/routes/memory.rs` — `/memory` GET and `/memory/refresh` POST
- `mimir-server/src/routes/chat.rs` — system prompt memory injection
- `mimir-core/src/scheduler.rs` — unified background scheduler

See `docs/memory-system.md` for the canonical technical walkthrough and `docs/wiki/memory.md` for the user-facing guide.
