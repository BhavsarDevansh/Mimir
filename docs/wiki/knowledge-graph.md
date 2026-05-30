# Knowledge Graph

The **Knowledge Graph** is Mimir's long-term memory. It stores facts about the user, their world, and their preferences in a structured, queryable database that grows more useful over time.

---

## What It Stores

- **Entities** — People, places, events, objects, concepts, organisations, and activities.
- **Facts** — Relationships between entities (e.g. "Alice works at Acme Corp") with validity dates and confidence scores.
- **Sources** — Where each fact came from (email, calendar, message, user edit, etc.).
- **Preferences** — Learned user preferences (notification style, calendar behaviour, privacy choices) with confidence and provenance.

---

## How Facts Work

Facts are **temporal edges** in the graph:

- A fact has a *subject* (who), a *predicate* (what), and an *object* (whom/what).
- Facts can have `valid_from` and `valid_until` timestamps, so Mimir knows "Alice lived in London" was true from 2018 to 2022 without contradicting "Alice lives in Berlin" from 2023 onwards.
- Every fact carries a **confidence score** (0.0–1.0) derived from source quality and inference chain depth. Confidence is calculated in Rust, never guessed by an LLM.

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
