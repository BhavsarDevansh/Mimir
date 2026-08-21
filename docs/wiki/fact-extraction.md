# How Mimir Learns Facts

The fact-extraction pipeline processes chat input to extract and store facts as structured subject-predicate-object triples in the knowledge graph. This page explains how the process works, what gets stored, and how you stay in control.

**Note:** Learning is LLM-orchestrated. The conversational LLM calls the `remember` tool during the chat turn to persist facts it judges worth keeping, so extraction runs inline as part of the same turn rather than as a separate post-response background pass. Because `remember` is a tool call within the turn, it may add a small amount of latency before the reply is finalised, and Mimir no longer learns from chitchat (Issue #137). The Librarian Agent and its background extraction pipeline remain available as an on-demand library API but are no longer auto-triggered every turn.

## Fact Quality

The extraction pipeline applies Rust-side normalisation and splitting to improve the quality of extracted facts:

- **Predicate resolution**: The LLM's relationship type is resolved through the alias table (the single source of truth). Common synonyms map to canonical names — for example, `attended` → `studied_at`, `hobbies` → `hobby` — purely from seeded aliases, with no hardcoded synonym list in code. An unknown predicate is rejected on the conversational path instead of being auto-registered as a new canonical type (issue #401).
- **List splitting**: When the LLM outputs a single fact with a comma-separated list (e.g., `hobby → "Geopolitics, Software Development, Tech"`), the pipeline automatically splits it into three independent facts. This applies to the multi-valued predicates (`hobby`, `likes`, `skill`, family relations, and the `favourite_<thing>` family such as `favourite_movie`), so a list of favourite films or pets is stored as separate facts rather than one comma-joined value.
- **Deduplication**: Before inserting a new fact, the pipeline checks if an identical active fact already exists. If so, it increments the confidence instead of creating a duplicate.

## Shared with connectors

The resolve → confidence → sensitivity-gate → insert steps are not conversation-specific. They live in a single shared function, `mimir_knowledge::normalize::normalize_and_insert`, that both the chat `remember` path and service connectors call. Connectors build the same `NormalizedFact` values from their items and supply a connector `Provenance`, so facts learned from your email, calendar, or photos get the identical confidence scoring, corroboration, supersession, and sensitivity gating as facts you tell Mimir directly — including cross-source corroboration, where the same fact reported by two connectors is merged into one knowledge-graph entry with boosted confidence rather than duplicated. Connector predicates are first-class ontology: every predicate the connectors emit (e.g. `has_event` for calendar entries, `attending`, `took_photo_at`, `has_flight`) is seeded as a canonical predicate with its own description and subject/object constraints, so a connector sync never invents a new predicate on the fly (issue #412).

## What Gets Extracted

Mimir looks for **subject-predicate-object** triples in your messages:

> "My favourite colour is blue."
>
> → Subject: you, Predicate: favourite_colour, Object: blue

## Learning Modes

Not everything you say is treated the same way:

| Mode | Example | Confidence | Overwrites? |
|------|---------|-----------|-------------|
| **Explicit** | "My favourite colour is blue." | 1.0 (certain) | Yes — replaces old fact |
| **Casual** | "Blue is a nice colour." | 0.30 (tentative) | No — coexists with explicit |
| **Correction** | "Actually, it's green, not blue." | 1.0 | Yes — old fact is corrected |

## Temporal Awareness

Mimir understands when facts change over time:

- **"I moved to London last month"** → The "lives_in" fact gets a start date. Your previous address is kept with an end date.
- **"My favourite colour has always been green"** → The old fact is marked as incorrect and archived.

## Sensitive Facts

Some facts are too important to store without asking:

- Health conditions and allergies
- Financial details
- Relationship status
- Religious or political beliefs

Mimir uses a two-stage check. The AI flags potential sensitive facts, but a deterministic Rust validation layer has the final say — it checks the fact's catalogue category and object text against a known sensitive set. This prevents benign preferences like "I don't like chihuahuas" or "I live in a small flat" from being flagged as sensitive just because the AI was overly cautious.

When Mimir confirms a fact is sensitive, it stores it as **pending confirmation** and asks you:

> ⚠️ I learned: You have a **PEANUT ALLERGY**. This is a sensitive health fact. Please confirm it is correct.
>
> [Confirm] [Reject]

- **Confirmed:** Stored permanently with full confidence.
- **Rejected:** Deleted immediately.
- **Ignored:** Auto-deleted after 7 days.

## How Confidence Works

Confidence is calculated by Mimir itself, not the AI. It depends on:

- How directly you stated the fact (explicit vs casual)
- Whether multiple sources agree (future feature)
- Whether the fact was inferred by rules (lower confidence)

## Your Control

You can always:
- View all stored facts
- Confirm or reject pending sensitive facts
- Edit or delete any fact
- Export everything to Markdown

### Recent Improvements (v0.40.3)

- **Stronger predicate normalisation**: The extraction pipeline now recognises more LLM variations such as `name`, `nickname`, `favorite_food`, `color`, and `colour`, mapping them to canonical forms automatically. Leading and trailing whitespace is also stripped before normalisation.
- **Expanded list splitting**: Multi-value predicates like `has_pets`, `has_child`, `has_parent`, `has_sibling`, and `has_partner` are now eligible for comma-separated list splitting, improving fact granularity.
- **Better error reporting**: The `remember` tool now surfaces full error messages in its output, making it easier to diagnose extraction failures.
- **Empty-message guard**: The background extraction task no longer spawns for empty or whitespace-only chat messages.

### Predicate Resolution Is Data-Driven (v0.50.0)

As of issue #136, Mimir no longer ships a hardcoded synonym map for relationship types. Every fact's predicate is resolved through the `relationship_type_aliases` table (seeded by migrations `036`/`037`), so `attended` still resolves to `studied_at`, `hobbies` to `hobby`, and so on. To teach Mimir a new synonym, register an alias against the canonical relationship type instead of waiting for a code change.

### Predicates Are Allow-Listed (v0.126.0)

As of issue #401, the conversational extraction path enforces a Rust-side canonical predicate allow-list: the LLM must use one of the seeded predicates (or a registered alias), and anything else — for example an invented `moved_into` — is rejected with a clear error instead of being silently stored as a new predicate. This keeps the predicate vocabulary stable so synonyms corroborate and supersede each other correctly, and it means the memory renderer never falls back to a bare invented verb. The prompt-instructed `favourite_<thing>` family (e.g. `favourite_movie`) remains available. One malformed predicate never blocks the rest of a batch — the error is reported and the other facts are still stored.
