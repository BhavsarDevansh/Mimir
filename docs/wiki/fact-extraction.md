# How Mimir Learns Facts

The fact-extraction pipeline processes chat input to extract and store facts as structured subject-predicate-object triples in the knowledge graph. This page explains how the process works, what gets stored, and how you stay in control.

**Note:** The extraction pipeline is automatically triggered after every non-incognito chat interaction. Facts are extracted in the background without delaying your response.

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
