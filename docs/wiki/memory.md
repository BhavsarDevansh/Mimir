# Memory

---

## Phase 2 Update (2026-06-06) — Now Default

Mimir uses a **dynamic, knowledge-graph-backed memory system**. Facts are now stored in SQLite and ranked on demand by a Rust scoring engine.

### How Memory Works Now

Instead of reading a text file, Mimir:

1. Queries the Knowledge Graph for facts about you
2. Scores each fact using a weighted formula (confidence, category, recency, priority, centrality)
3. Selects the top facts that fit within a 2500-character budget (default top-N for condensation hash is 500)
4. Renders them as concise plain text (or sends them to an LLM for condensation)
5. Caches the result in `system_state` for instant retrieval

### Regeneration Triggers

Memory is regenerated **on demand** when facts change, not on a fixed timer. The background scheduler ensures condensation only runs during LLM downtime so it never slows down your conversations.

You can also force regeneration immediately:

```bash
mimir memory --refresh
```

### What Affects Your Memory Ranking

| Factor | What it means |
|--------|---------------|
| Confidence | How certain Mimir is (based on source quality and corroboration) |
| Category | Identity facts (1.0) rank higher than hobbies (0.55) |
| Temporal boost | Upcoming birthdays and appointments get a recency boost |
| Priority | Critical facts (partner, allergies) get a 2× multiplier |
| Centrality | Facts about well-connected entities (people you mention often) rank higher |

### Configuration

```toml
[memory]
char_limit = 2500
condensation_top_n = 500
```

### What This Means for You

- No need to manually edit memory — Mimir builds your memory automatically from conversations
- The memory block is always current (regenerated when facts change, gated by scheduler)
- You can still inspect what Mimir knows via `mimir memory` and `mimir kg query`
- If you want a fact pinned or deprioritised, that will be supported in a future update

### Your Name

When you run `mimir init` and provide your name, Mimir stores it as a fact in the knowledge graph during the next server startup. This means your name is available in memory right away — no need to tell Mimir again in chat.
