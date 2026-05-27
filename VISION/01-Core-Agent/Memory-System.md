# memory.md — The Working Memory

## Philosophy

`memory.md` is Mimir's executive summary. A small, curated, always-hot cache of the most critical facts, system context, and pointers to deeper knowledge. It is injected into every system prompt so that even the very first interaction is grounded in what Mimir already knows.

Think of it as the agent's **index card** — not the full encyclopedia, but the page numbers of the most important chapters.

## Role in the Architecture

```
User Query
    ↓
System Prompt (injected memory.md + personality + base instructions)
    ↓
LLM reasons about the query
    ↓
If memory.md alone is insufficient → Query Knowledge Graph, Connectors, or Reasoning Engine
```

`memory.md` reduces the need to hit the Knowledge Graph for trivial lookups. The LLM already knows the user's name, their current city, their primary email, their favourite editor, their upcoming flight. This saves tokens, latency, and complexity.

## Contents

### Tier 1: Identity and Context (Always Present)
These are so fundamental that they are always included:

```
Devansh, born [DD MMM YYYY].
Lives in [CITY].
Software Developer (C# Fullstack).
```

### Tier 2: Active Projects and Environment
Current work context the agent needs to function effectively:

```
Active projects: mimir (~/code/mimir; cargo test,run), librechat (~/code/librechat; trunk serve).
```

### Tier 3: Critical Preferences
Preferences that affect every interaction:

```
Prefers transparent communication, important-only proactivity, no medical topics in public.
Calendar: auto-add flights (granted 2025-05-10).
```

### Tier 4: Temporal Facts (Auto-Rotating)
Time-sensitive facts that are important *now* but may not be in a month:

```
Upcoming: flight JL043 Tokyo May25 11:00; Priya birthday Jun15.
Recently moved to Berlin (March 2026).
```

### Tier 5: Knowledge Graph Pointers
Pointers to where deeper information lives, so the agent knows what to query:

```
KB: travel 12 destinations, work 2 positions, relationships (Alice), preferences 23 entries.
```

## Format

The default template is a free-form agent scratchpad with no rigid sections:

```markdown
Mimir memory [0/2500]

No memories yet.
```

The agent writes compact, self-contained notes (one thought per line or bullet). It groups related facts organically and decides its own structure. Example populated memory:

```markdown
Mimir memory [247/2500]

Devansh, born [DD MMM YYYY].
Lives in [CITY].
Software Developer (C# Fullstack).
Married to [WIFE]; her birthday [DD MM YYYY].
```

Agents use `replace` on the exact existing note to update it, preventing duplication.

## Size and Budget

**Hard limit:** 2,500 characters (~900 tokens)

This is intentionally small. The goal is to be cheap and fast, not comprehensive. If the full context is needed, the agent queries the Knowledge Graph.

**Budget allocation (suggested):**
- Identity + System: ~400 chars
- Active Projects: ~500 chars
- Preferences: ~400 chars
- Temporal: ~400 chars
- KB Pointers: ~400 chars
- Overhead (headers, delimiters): ~400 chars
- **Total: ~2,500 chars**

## Auto-Management

The agent manages `memory.md` automatically using the `memory` tool:

```rust
enum MemoryAction {
    Add { content: String },
    Replace { old_text: String, content: String },
    Remove { old_text: String },
}
```

### When the Agent Adds to memory.md
- New environment discovered (new project, new machine)
- Critical preference learned (user explicitly stated)
- Temporal fact becomes important (upcoming event within 30 days)
- New KB pointer needed (new category of facts being stored)

### When the Agent Removes from memory.md
- Temporal fact expires (event passed)
- Project no longer active (no activity in 90 days)
- Preference overridden (user changed mind)
- Environment changed (moved cities, switched editors)

### Consolidation
When memory approaches the limit, the agent consolidates:
```
Before: 3 separate entries
  "User runs macOS 14"
  "User has M3 MacBook Pro"
  "User uses zsh with oh-my-zsh"

After: 1 consolidated entry
  "macOS 14, M3 MacBook Pro, zsh with oh-my-zosh"
```

## Relationship to Knowledge Graph

```
memory.md          Knowledge Graph
     │                   │
     │  ┌───────────────┐ │
     └──┤ Hot Cache     ├─┘
        │ (2,500 chars) │
        └───────────────┘
              │
              ▼
        ┌───────────────┐
        │ Full Graph    │
        │ (millions of  │
        │ facts)        │
        └───────────────┘
```

**Query order:**
1. memory.md (injected in prompt, instant, free)
2. Knowledge Graph (SQLite query, ~1-5ms, cheap)
3. Connectors (API calls, ~100-500ms, may cost tokens)
4. Reasoning Engine (multi-thread, ~1-10s, most expensive)

**memory.md is always queried first because it's already in context.** Only if the answer isn't there does the agent escalate.

## Frozen Snapshot

Like Hermes Agent, memory.md is loaded once at session start and remains frozen for the duration of the conversation. This preserves the LLM's prefix cache for performance.

Changes made during a session are written to disk immediately but do not appear in the active system prompt until the next session starts.

## File Location

```
~/.config/mimir/memory.md
```

## Example: Full memory.md

```markdown
Mimir memory [1,890/2500]

Devansh, born [DD MMM YYYY].
Lives in [CITY].
Software Developer (C# Fullstack).
Married to [WIFE]; her birthday [DD MM YYYY].
Uses macOS 14 on M3 MBP, zsh+oh-my-zsh, VSCode+Vim.
Active projects: mimir (~/code/mimir; cargo test,run), librechat (~/code/librechat; trunk serve).
Prefers transparent communication, important-only proactivity, no medical topics in public.
Calendar: auto-add flights (granted 2025-05-10); DND 22:00–08:00.
Upcoming: flight JL043 Tokyo May25 11:00; Priya birthday Jun15.
Recently moved to Berlin (March 2026).
KB: travel 12 destinations, work 2 positions, relationships (Alice), preferences 23 entries, health 2 allergies.
```

## Configuration

```toml
[memory]
enabled = true
char_limit = 2500
auto_manage = true
frozen_snapshot = true  # Load once per session

# What categories to include
include_identity = true
include_projects = true
include_preferences = true
include_temporal = true
include_kb_pointers = true

# Temporal horizon (days into future to include events)
temporal_horizon = 30

# Project staleness threshold (days without activity)
project_staleness = 90
```
