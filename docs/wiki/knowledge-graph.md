# Knowledge Graph

The **Knowledge Graph** is Mimir's long-term memory — a structured “second brain” that stores facts about you, your world, and your preferences in a queryable database that grows more useful the longer you use it.

It is distinct from the condensed memory that `mimir memory` prints. The condensed memory is a **short, ranked summary** rendered *from* the Knowledge Graph to fit inside the agent's system prompt each turn. The Knowledge Graph itself is the fully structured store: every entity, every fact with its temporal bounds, confidence, provenance, and inferred relationships. The old flat `memory.md` file has been retired in favour of this on-demand rendering — there is no separate text file to keep in sync.

---

## What It Stores

- **Entities** — People, places, events, objects, concepts, organisations, activities, and dates.
- **Facts** — Relationships between entities (e.g. "Alice works at Acme Corp") with validity dates and confidence scores.
- **Entity Aliases** — Alternative names (nicknames, maiden names, abbreviations) that improve search and reduce duplicates.
- **Events & Reminders** — A lifecycle + recurrence overlay on facts. A future-dated fact is a one-time event; a recurring fact (e.g. a birthday) is a recurring event; a fact requiring action is a task. Upcoming events surface automatically in your memory (see [Events & Reminders](events-and-reminders.md)).
- **Entity Locations** — Addresses, coordinates, and timezones with validity windows.
- **Sources** — Where each fact came from (email, calendar, message, user edit, etc.).
- **Preferences** — Learned user preferences (notification style, calendar behaviour, privacy choices) with confidence and provenance.

---

## How Facts Work

Facts are **temporal edges** in the graph:

- A fact has a *subject* (who), a *predicate* (what), and an *object* (whom/what).
- Facts can have `valid_from` and `valid_until` timestamps, so Mimir knows "Alice lived in London" was true from 2018 to 2022 without contradicting "Alice lives in Berlin" from 2023 onwards.
- Every fact carries a **confidence score** (0.0–1.0) derived from source quality and inference chain depth. Confidence is calculated in Rust, never guessed by an LLM.
- Predicate constraints validate subject/object type combinations at insert time (e.g. `born_on` requires a `DateTime` object). Predicates without seeded constraints accept any entity types, and facts with literal (non-entity) objects always pass; a violating combination is rejected with a clear error instead of being stored.

### Inference

Mimir derives new facts from existing ones using a Rust-native inference engine — no LLM, no guessing. For example, if it knows “Alice visited Rome” and “Rome is in Italy”, it can infer “Alice visited Italy” with a lower confidence that decays with chain depth. Inferred facts are linked back to their source facts so they update automatically when the underlying facts change. See [Inference Rules](inference-rules.md).

---

## How Entities Work

### Search & Aliases

When you mention a name, Mimir tries to resolve it in three steps:

1. **Exact name match** — case-insensitive match on the primary entity name.
2. **Exact alias match** — case-insensitive match on any registered alias.
3. **Fuzzy search** — SQLite FTS5 full-text search with a relevance score, used only when the score is high enough (≥ 0.9) to trust.

Resolution is **type-aware**: only entities matching the declared type (Person, Place, Organization, …) are considered, so "Apple" mentioned as a concept is never confused with the company "Apple Inc". If nothing matches — including a weak fuzzy hit — Mimir creates a new entity with the declared type. You can add or remove aliases at any time via the API; aliases are learned explicitly (for example through a `preferred_name` fact), not auto-guessed from fuzzy matches.

### Relationship Types

Facts connect entities with a **relationship type** (sometimes called a predicate), such as `works_at`, `resides_in`, or `has_sibling`. Relationship types form a controlled vocabulary:

- The system keeps a canonical list of relationship type names.
- Each canonical name can have **aliases** (synonyms). For example, `studied_at` might have aliases `attended` and `alumni_of`.
- Aliases are normalized (lowercase, spaces become underscores) so `Works At`, `works_at`, and `works at` all resolve to the same relationship type.
- When a fact is extracted from a chat turn, the relationship type is resolved through the alias table before the fact is stored. If the LLM says "attended", Mimir stores it under the canonical `studied_at` type because `attended` is a registered alias.
- New canonical types automatically register their normalized name as a self-alias, so the alias table is the single lookup source for every relationship type.
- To keep resolution unambiguous, a canonical name cannot be created if it would shadow an existing alias, and an alias cannot be created if it would shadow an existing canonical name.

- Relationship types also form a **hierarchy** (a directed acyclic graph). A type can sit under parent types — for example `studied_at` and `graduated_from` can be children of an `education` type. When the agent queries a type it can expand to the whole **subtree**, so asking about "education" finds `studied_at`, `graduated_from`, and any other descendants without the agent needing to know every type name (see `kg_query` with `include_subtree`).
- Redundant verbs are consolidated: `based_in` and `lived_in` are aliases of `resides_in` (current and previous residence are one relation with different time bounds), and `is_in` is an alias of `located_in`. The abstract parents `employment`, `education`, `residence`, and `containment` are query-only subtree roots — they are never stored on facts.

### Deduplication

Mimir automatically detects and resolves duplicate entities:

- **Exact duplicates** — same name (case-insensitive). The entity with more facts survives; the other is merged in. All facts and aliases are preserved.
- **Overlapping aliases** — two entities sharing an alias are flagged in a review queue so you can decide whether to merge them.
- **Semantic duplicates** — the nightly optimization run uses the LLM to judge whether two entities (or near-identical facts) with different names are the same real-world thing. High-confidence matches are auto-merged; uncertain ones are queued for your review (see [Nightly Optimization](nightly-optimization.md)).

---

## Storage

- **Backend:** SQLite single-file database.
- **Location:** `~/.local/share/mimir/knowledge.db`
- **Access:** Only the Mimir daemon reads or writes the database. CLI commands talk to the daemon via HTTP; they never touch the DB directly.
- **Search:** Full-text search over entity names and aliases via SQLite FTS5.

---

## Inspecting the Knowledge Graph

The `mimir kb` commands let you query, browse, edit, and forget facts, and the `mimir memory` command renders the condensed summary. All commands talk to the daemon over HTTP and support `--json` for scriptable output. See [CLI Commands](cli-commands.md) for the full reference.

```bash
mimir kb query "Alice"          # All facts about Alice
mimir kb show 42               # Full detail for one fact
mimir kb browse --entity "Alice" --depth 2
mimir kb profile               # A biography from your top facts
mimir kb audit --entity Alice  # Audit log for Alice's facts
mimir kb optimization --status # Nightly optimization status
```

---

## Relationship to the Wider System

- **Chat** — the agent calls `kg_query`, `kg_related`, and `kg_search` tools to pull facts on demand, and a short condensed summary is injected into the system prompt each turn.
- **Learning** — the server-side `remember.chat` background hook extracts facts after each non-incognito turn; all validation, confidence, and insertion logic is deterministic Rust (see [How Mimir Learns Facts](fact-extraction.md) and [Background Hooks](hooks.md)).
- **Memory** — `mimir memory` renders a ranked condensation of the Knowledge Graph, not a separate file (see [Memory](memory.md)).
- **Nightly optimization** — keeps the graph healthy: dedup, contradiction resolution, confidence recalculation, and cleanup (see [Nightly Optimization](nightly-optimization.md)).

---

## Privacy & Trust

- All data stays on your device. No cloud intermediary.
- Every fact records its source, so you can trace how Mimir learned something.
- Facts are only removed when a higher-confidence counter-fact exists and the dispute has been dormant for 30 days. Old facts are preserved as historical context, not deleted.
- Deleting an entity that has attached facts is blocked, preventing accidental loss of knowledge.
