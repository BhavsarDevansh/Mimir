# How Confidence Works in Mimir

## What Is Confidence?

Confidence is a number from 0.0 to 1.0 that tells you how strongly Mimir believes a fact. Unlike many AI systems, Mimir's confidence is **not a guess from a language model**. It is calculated from the graph itself — who said what, how many sources agree, and how far the fact was inferred from raw evidence.

## How Facts Get Their Initial Confidence

When Mimir learns something, it looks at **how** it learned it:

| How it was learned | Confidence | Why |
|---|---|---|
| You said it directly | 1.0 | You are the authority on yourself. |
| System operation | 1.0 | Internal, deterministic. |
| Casual mention in chat | 0.30 | Passing thought, not an assertion. |
| Imported from a file | 0.80 | Bulk data, verified later. |
| Extracted from Gmail | 0.85 | Per-connector reliability score. |
| Extracted from Calendar | 0.90 | Calendar data is usually accurate. |
| Reasoned by inference | Varies | Depends on parent facts and depth. |

## Inference: "Spreadsheet Cell" Confidence

Inferred facts work like formula cells in a spreadsheet. Their confidence depends on the confidence of the facts they were inferred from.

Example:
- You said "I like Italian food" (confidence 1.0)
- You said "I like basil" (confidence 1.0)
- Mimir infers: "You probably like basil pesto"

The inference confidence formula considers:
1. **Parent confidence** — how strong the source facts are
2. **Sign** — whether each parent supports (+) or opposes (−) the conclusion
3. **Depth** — how many inference hops away from raw evidence (each hop multiplies by 0.8)
4. **Breadth** — how many independent parents support it (more parents = higher confidence)

## What Happens When Facts Change

When a source fact is forgotten or corrected:
- **Inferred children are recalculated** automatically.
- If you remove a fact that was *supporting* an inference, the inference's confidence drops.
- If you remove a fact that was *opposing* an inference, the inference's confidence rises.
- **Explicit facts never lose confidence** — they were true when you said them.

## Corroboration (#79)

When Mimir learns the same claim from a second **independent** source, it does not create a duplicate fact. Instead the existing fact gains a new source row (provenance) and, if it is non-explicit and non-inferred, its confidence rises by `0.05` per corroborating source, capped at `0.95`.

- "Same claim" means same subject + predicate + object with **overlapping** time ranges. Two facts about different time periods (e.g. "lived in London last year" vs "last month") are separate timeline facts, not corroboration.
- An explicit statement (something you said directly) always **supersedes** rather than corroborates, so your direct input wins.
- Corroborating an **explicit** fact adds the source but does not change confidence (it is already 1.0).
- Corroborating an **inferred** fact adds the source but does not change confidence — inferred confidence is structural (derived from its parents).
- A re-statement from the *same* source (same source type, connector, and reference) is a no-op, not corroboration.

When a fact's confidence changes, the change cascades to every fact inferred from it, all the way down the inference graph, so derived facts stay accurate. The cascade is comprehensive (no depth limit) and correct even when a fact is reachable through multiple inference paths — it is recalculated once per updated parent and ends up with the right value.

- Corroboration never **lowers** confidence: once a fact reaches the `0.95` cap, further corroboration keeps the current value.
- Corroboration also clears a fact's "needs recalculation" flag even at the cap, so a corroborated fact is no longer treated as stale.

## Keeping Confidence Fresh

Some facts are flagged as needing recalculation (`stale_confidence`). The nightly optimisation pass recalculates each stale fact from its parents and clears the flag, then cascades the result to inferred descendants — all in one transaction. This means a stale fact itself gets updated, not just its children, so nothing stays "needs recalculation" forever.

## Connector Reliability

Each connector (Gmail, Calendar, etc.) has a reliability score. When Mimir extracts a fact from that connector, the fact inherits the connector's current score.

If you correct a fact from Gmail, Gmail's score drops slightly. If a Gmail fact is later confirmed by another source, Gmail's score rises slightly. This only affects **future** facts — old facts keep the score they had when they were created.

## No Decay, No Guessing

Mimir does **not** reduce confidence just because a fact is old. A fact you told Mimir five years ago is just as trustworthy today, unless you explicitly change it. Time does not erode truth.
