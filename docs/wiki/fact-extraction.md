# How Mimir Learns Facts

The fact-extraction pipeline processes chat input to extract and store facts as structured subject-predicate-object triples in the knowledge graph. This page explains how the process works, what gets stored, and how you stay in control.

**Note:** The extraction pipeline is automatically triggered after every non-incognito chat interaction. Facts are extracted in the background without delaying your response. Additionally, the LLM has a `remember` tool it can call proactively during conversation to write facts directly.

## Fact Quality

The extraction pipeline applies Rust-side normalisation and splitting to improve the quality of extracted facts:

- **Predicate resolution**: The LLM's relationship type is resolved through the alias table (the single source of truth). Common synonyms map to canonical names — for example, `attended` → `studied_at`, `hobbies` → `hobby` — purely from seeded aliases, with no hardcoded synonym list in code. An unknown predicate is auto-registered as a new canonical type.
- **List splitting**: When the LLM outputs a single fact with a comma-separated list (e.g., `hobby → "Geopolitics, Software Development, Tech"`), the pipeline automatically splits it into three independent facts.
- **Deduplication**: Before inserting a new fact, the pipeline checks if an identical active fact already exists. If so, it increments the confidence instead of creating a duplicate.

## What Gets Extracted

Mimir looks for **subject-predicate-object** triples in your messages:

> "My favourite colour is blue."  
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

When Mimir detects a sensitive fact, it stores it as **pending confirmation** and asks you:

> ⚠️ I learned: You have a **PEANUT ALLERGY**. This is a sensitive health fact. Please confirm it is correct.  
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

As of issue #136, Mimir no longer ships a hardcoded synonym map for relationship types. Every fact's predicate is resolved through `ensure_relationship_type`, which consults the `relationship_type_aliases` table (seeded by migrations `036`/`037`) and auto-registers unknown predicates as new canonical types. Resolution errors are tolerated per-fact, so one malformed predicate won't block the rest of a batch. End-user behaviour is unchanged: `attended` still resolves to `studied_at`, `hobbies` to `hobby`, and so on. To teach Mimir a new synonym, register an alias against the canonical relationship type instead of waiting for a code change.
