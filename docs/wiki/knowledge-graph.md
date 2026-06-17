# Knowledge Graph

The **Knowledge Graph** is Mimir's long-term memory. It stores facts about the user, their world, and their preferences in a structured, queryable database that grows more useful over time.

---

## What It Stores

- **Entities** — People, places, events, objects, concepts, organisations, activities, and dates.
- **Facts** — Relationships between entities (e.g. "Alice works at Acme Corp") with validity dates and confidence scores.
- **Entity Aliases** — Alternative names (nicknames, maiden names, abbreviations) that improve search and reduce duplicates.
- **Entity Dates** — Birthdays, anniversaries, creation dates, and custom dates. Dates can be one-time or recurring (daily, weekly, monthly, yearly).
- **Entity Locations** — Addresses, coordinates, and timezones with validity windows.
- **Sources** — Where each fact came from (email, calendar, message, user edit, etc.).
- **Preferences** — Learned user preferences (notification style, calendar behaviour, privacy choices) with confidence and provenance.

---

## How Facts Work

Facts are **temporal edges** in the graph:

- A fact has a *subject* (who), a *predicate* (what), and an *object* (whom/what).
- Facts can have `valid_from` and `valid_until` timestamps, so Mimir knows "Alice lived in London" was true from 2018 to 2022 without contradicting "Alice lives in Berlin" from 2023 onwards.
- Every fact carries a **confidence score** (0.0–1.0) derived from source quality and inference chain depth. Confidence is calculated in Rust, never guessed by an LLM.
- Predicate constraints validate subject/object type combinations at insert time (e.g. `born_on` requires a `DateTime` object).

---

## How Entities Work

### Search & Aliases

When you mention a name, Mimir tries to resolve it in three steps:

1. **Exact name match** — case-insensitive match on the primary entity name.
2. **Exact alias match** — case-insensitive match on any registered alias.
3. **Fuzzy search** — SQLite FTS5 full-text search with a relevance score.

You can add or remove aliases at any time via the API.

### Relationship Types

Facts connect entities with a **relationship type** (sometimes called a predicate), such as `works_at`, `lives_in`, or `has_sibling`. Relationship types form a controlled vocabulary:

- The system keeps a canonical list of relationship type names.
- Each canonical name can have **aliases** (synonyms). For example, `studied_at` might have aliases `attended` and `alumni_of`.
- Aliases are normalized (lowercase, spaces become underscores) so `Works At`, `works_at`, and `works at` all resolve to the same relationship type.
- When a fact is extracted from a chat turn, the relationship type is resolved through the alias table before the fact is stored. If the LLM says "attended", Mimir stores it under the canonical `studied_at` type because `attended` is a registered alias.
- New canonical types automatically register their normalized name as a self-alias, so the alias table is the single lookup source for every relationship type.
- To keep resolution unambiguous, a canonical name cannot be created if it would shadow an existing alias, and an alias cannot be created if it would shadow an existing canonical name.

### Deduplication

Mimir automatically detects and resolves duplicate entities:

- **Exact duplicates** — same name (case-insensitive). The entity with more facts survives; the other is merged in. All facts and aliases are preserved.
- **Overlapping aliases** — two entities sharing an alias are flagged in a review queue so you can decide whether to merge them.
- **Semantic duplicates** — future work (#50) will use the LLM to judge whether two entities with different names are the same real-world thing.

---

## Storage

- **Backend:** SQLite single-file database.
- **Location:** `~/.local/share/mimir/knowledge.db`
- **Access:** Only the Mimir daemon reads or writes the database. CLI commands talk to the daemon via HTTP; they never touch the DB directly.
- **Search:** Full-text search over entity names and aliases via SQLite FTS5.

---

## Future Commands (Planned)

```bash
# Inspect the knowledge graph
mimir kb entities        # List recent entities
mimir kb facts Alice     # Facts about "Alice"
mimir kb search Paris    # FTS search across entities
mimir kb contradictions  # Highlight disputed facts
mimir kb optimize        # Trigger nightly dedup + confidence recalc
```

---

## Privacy & Trust

- All data stays on your device. No cloud intermediary.
- Every fact records its source, so you can trace how Mimir learned something.
- Facts are only removed when a higher-confidence counter-fact exists and the dispute has been dormant for 30 days. Old facts are preserved as historical context, not deleted.
- Deleting an entity that has attached facts is blocked, preventing accidental loss of knowledge.
