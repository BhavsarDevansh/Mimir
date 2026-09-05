# How Mimir Learns Facts

The fact-extraction pipeline processes chat input to extract and store facts as structured subject-predicate-object triples in the knowledge graph. This page explains how the process works, what gets stored, and how you stay in control.

**Note:** Learning is hook-driven. After each non-incognito chat turn, the server-side `remember.chat` background hook runs the extraction pipeline over the accumulated conversation (Issue #386). The hook is debounced per session and waits for the LLM pool to be idle, so extraction runs in the background after your reply is finalised and a burst of messages becomes one extraction. The `remember` tool was removed from the model's tool set, so learning no longer depends on the model deciding to call it (Issue #137). The Librarian Agent and its background extraction pipeline remain available as an on-demand library API.

## Fact Quality

The extraction pipeline applies Rust-side normalisation and splitting to improve the quality of extracted facts:

- **Predicate resolution**: The extraction schema uses a closed `relationship_type` enum generated from the database taxonomy. Rust then resolves that leaf through controlled aliases — for example, `attended` → `studied_at`, `likes` → `prefers` — and rejects unknown predicates instead of auto-registering them (issues #401 and #468). A fact that reaches normalization with an unknown predicate is staged for review rather than silently discarded.
- **Category visibility**: The extraction prompt includes the complete DB-driven category tree, so categories such as `Sweet Foods` and `Board Games` are visible even when they are grandchildren in the taxonomy. The model can therefore choose the most specific category available, while Rust still validates the selected IDs and applies a deterministic category fallback when needed.
- **List splitting**: When the LLM outputs a single fact with a comma-separated list (e.g., `hobby → "Geopolitics, Software Development, Tech"`), the pipeline automatically splits it into three independent facts. This applies to the multi-valued predicates (`hobby`, `prefers`, `skill`, family relations), so a list of favourite films or pets is stored as separate facts rather than one comma-joined value.
- **Deduplication**: Before inserting a new fact, the pipeline checks if an identical active fact already exists. If so, it increments the confidence instead of creating a duplicate.

## Shared with connectors

The resolve → confidence → sensitivity-gate → insert steps are not conversation-specific. They live in a single shared function, `mimir_knowledge::normalize::normalize_and_insert`, that both the chat learning path (`remember.chat` hook) and service connectors call. Connectors build the same `NormalizedFact` values from their items and supply a connector `Provenance`, so facts learned from your email, calendar, or photos get the identical confidence scoring, corroboration, supersession, and sensitivity gating as facts you tell Mimir directly — including cross-source corroboration, where the same fact reported by two connectors is merged into one knowledge-graph entry with boosted confidence rather than duplicated. Connector predicates are first-class ontology: every predicate the connectors emit (e.g. `has_event` for calendar entries, `attending`, `took_photo_at`, `has_flight`) is seeded as a canonical predicate with its own description and subject/object constraints, so a connector sync never invents a new predicate on the fly (issue #412).

## What Gets Extracted

Mimir looks for **subject-predicate-object** triples in your messages:

> "I prefer blue."
>
> → Subject: you, Predicate: prefers, Object: blue

## Learning Modes

Not everything you say is treated the same way:

| Mode | Example | Confidence | Overwrites? |
|------|---------|-----------|-------------|
| **Explicit** | "I prefer blue." | 1.0 (certain) | Yes — replaces old fact |
| **Casual** | "Blue is a nice colour." | 0.30 (tentative) | No — coexists with explicit |
| **Correction** | "Actually, it's green, not blue." | 1.0 | Yes — old fact is corrected |

## Temporal Awareness

Mimir understands when facts change over time:

- **"I moved to London last month"** → The "lives_in" fact gets a start date. Your previous address is kept with an end date.
- **"My preferred colour has always been green"** → The old fact is marked as incorrect and archived.

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
- Whether multiple sources agree (cross-source corroboration boosts confidence)
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
- **Better error reporting**: Extraction failures now surface full error messages in the hook logs, making it easier to diagnose extraction failures.
- **Empty-message guard**: The background extraction task no longer spawns for empty or whitespace-only chat messages.

### Predicate Resolution Is Data-Driven (v0.50.0)

As of issue #136, Mimir no longer ships a hardcoded synonym map for relationship types. Every fact's predicate is resolved through the `relationship_type_aliases` table (seeded by migrations `036`/`037`), so `attended` still resolves to `studied_at`, `hobbies` to `hobby`, and so on. To teach Mimir a new synonym, register an alias against the canonical relationship type instead of waiting for a code change.

### Predicates Are Allow-Listed (v0.126.0)

As of issues #401 and #468, conversational extraction uses a closed `relationship_type` enum generated from the DB taxonomy, then resolves aliases and emits only a queryable leaf. Anything else — for example an invented `moved_into` — is staged for review instead of being silently stored as a new predicate. This keeps the predicate vocabulary stable so synonyms corroborate and supersede each other correctly. One malformed predicate never blocks the rest of a batch — the error is reported and the other facts are still stored.

As of issue #468, the email prose layer stages malformed or unrecognized facts in a durable review queue rather than dropping them. Connector status reports `facts_accepted`, `facts_dropped`, and `facts_staged`, so a vocabulary gap is visible instead of being hidden behind the item count. See [Closed Taxonomy and Fact Staging](closed-taxonomy-staging.md).
