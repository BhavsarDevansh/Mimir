# Memory System

## Overview

Mimir's memory system is a **knowledge-graph-backed, on-demand rendered memory block**. It replaced the legacy `memory.md` file-based system in v0.37.0 (Issue #111).

Instead of reading a static text file, Mimir:

1. Queries the Knowledge Graph (`mimir-knowledge`) for facts about the user
2. Scores each fact using a weighted formula: `confidence × category.memory_weight × temporal_boost × priority × centrality`
3. Selects top facts within a configurable character budget (default 2,500)
4. Renders them as a structured schema (`MemorySchema`) sent to an LLM for condensation into natural prose
5. Caches the condensed result in `system_state` (`key = "condensed_memory"`) for instant retrieval

## Identity Fact Seeding

When the server starts, it resolves the user entity from `[identity].name` in config. If the entity exists (or is created), the server seeds two identity facts automatically:

- `has_name` — the user's full name from `identity.name`
- `preferred_name` — the user's preferred name from `identity.preferred_name` (only if it differs from the full name)

These facts are categorised as Identity (category ID 110) so they rank at the top of the memory schema. Seeding is idempotent — if an active fact with the same predicate and literal already exists, it is skipped. This ensures Mimir knows the user's name immediately after initialization, without requiring a separate conversation to extract it.

## Architecture

### Fact Ranking & Selection

Implemented in `mimir-knowledge/src/queries/memory/`.

- **Identity facts** always rank first (name, pronouns, birthdate)
- **Temporal boost** increases score for upcoming events (birthdays, appointments) based on proximity
- **Priority** (`memory_priority_id`) gives critical facts a 2× multiplier
- **Centrality** boosts facts about well-connected entities (people mentioned often)
- **Fill algorithm**: Sort by score descending, greedily fill the character budget, truncate last entry with `…` if exceeded

### Memory Buckets

Every ranked fact lands in one of five buckets — `Identity`, `Upcoming`, `Relationships`, `Preferences`, or `General` — which controls the section it is rendered under. Only `Identity` affects fill order: the fill algorithm reserves a first phase for identity facts (up to ~200 chars with rollover) and then fills the remaining budget by score across `Upcoming`, `Relationships`, `Preferences`, and `General` together. Bucket classification is data-driven: migration `052` added a `memory_buckets` lookup table and a `categories.memory_bucket_id` column, backfilled from the taxonomy seeded in migration `031` (identity 100–199, upcoming 900–999, relationships 400–499, preferences 300–399 plus the preference-ish outliers 570/670/680/690/830/870, everything else general). When a fact spans multiple categories, the memory query classifies it into the bucket with the lowest id (`MIN(c.memory_bucket_id)`); the ids encode classification priority (Identity 1 > Upcoming 2 > Relationships 3 > Preferences 4 > General 5), not a per-bucket fill-priority schedule. `mimir-knowledge/src/queries/memory/ranking.rs` only maps a stored bucket id to the `MemoryBucket` enum, falling back to `General` for unset or unknown ids — there are no hard-coded category ranges in Rust to drift from the taxonomy. Categories created at runtime without an explicit bucket (`kb category add --memory-bucket-id`) classify as `General`.

### LLM Condensation Pipeline

Implemented in `mimir-knowledge/src/condensation.rs`.

- Builds a `MemorySchema` excluding upcoming and sensitive facts
- Computes a hash of the top-N stable facts (configurable, default 500)
- If the hash matches the stored hash, skips the LLM call (no-op)
- Otherwise, calls the LLM with a pure formatting prompt (no conditional logic, no decision-making)
- Validates output length against budget
- Falls back to deterministic Rust template rendering on LLM failure or oversize output
- Stores result in `system_state`

### Regeneration Triggers

Condensed memory is regenerated when:
- A fact is inserted/updated/deleted that ranks in top-N for memory inclusion (demand-driven via `BackgroundScheduler`)
- `mimir memory --refresh` is called explicitly (force-submit)
- Nightly optimization completes (confidence recalculation may re-rank facts)

The scheduler ensures condensation only runs during LLM downtime so it never competes with active chats.

### Context Injection

The condensed-memory system prompt is composed at session creation for non-incognito sessions, combined with an upcoming events section, and reused for the session's lifetime; incognito requests build a fresh prompt per request. The prompt phrasing is "Core facts about the user (condensed subset — not a complete picture; treat as starting context, not exhaustive)", signalling to the LLM that the subset is curated and it should use KG tools if it needs more.

## Configuration

```toml
[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
condensation_top_n = 500
```

## Files

- `mimir-knowledge/src/queries/memory/` — ranking (`ranking.rs`), scoring, budget fill (`build.rs`), rendering (`render.rs`)
- `mimir-knowledge/src/models/memory.rs` — `MemorySchema`, `MemoryBucket`, `RankedFact`
- `mimir-knowledge/src/condensation.rs` — LLM condensation pipeline
- `mimir-knowledge/src/queries/system_state.rs` — `condensed_memory` cache read/write
- `mimir-server/src/routes/memory.rs` — `/memory` GET and `/memory/refresh` POST
- `mimir-server/src/routes/chat.rs` — system prompt memory injection
- `mimir-core/src/scheduler.rs` — unified background scheduler
