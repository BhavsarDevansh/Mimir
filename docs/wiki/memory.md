
---

## Phase 2 Update (2026-06-06)

Mimir is transitioning from a static `memory.md` file to a **dynamic, knowledge-graph-backed memory system**. Facts are now stored in SQLite and ranked on demand by a Rust scoring engine.

### How Memory Works Now

Instead of reading a text file, Mimir:

1. Queries the Knowledge Graph for facts about you
2. Scores each fact using a weighted formula (confidence, category, recency, priority, centrality)
3. Selects the top facts that fit within a 2500-character budget
4. Renders them as concise plain text (or sends them to an LLM for condensation)
5. Caches the result in `system_state` for instant retrieval

### What Affects Your Memory Ranking

| Factor | What it means |
|--------|---------------|
| Confidence | How certain Mimir is (based on source quality and corroboration) |
| Category | Identity facts (1.0) rank higher than hobbies (0.55) |
| Temporal boost | Upcoming birthdays and appointments get a recency boost |
| Priority | Critical facts (partner, allergies) get a 2× multiplier |
| Centrality | Facts about well-connected entities (people you mention often) rank higher |

### What This Means for You

- No need to manually edit `memory.md` — Mimir builds your memory automatically from conversations
- The memory block is always current (regenerated when facts change)
- You can still inspect what Mimir knows via `mimir memory` and `mimir kg query`
- If you want a fact pinned or deprioritised, that will be supported in a future update
