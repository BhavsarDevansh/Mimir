# Learning Modes

## Philosophy
The Knowledge Graph learns through multiple channels, each with different reliability and overwrite rules. Not all learning is equal — explicit statements from the user override inferred facts, and sensitive facts require special confirmation.

## Learning Channels

### 1. Explicit Statement (Highest Priority)
The user directly tells the agent something about themselves.

**Trigger:** Direct assertions in natural language.
> "My favourite colour is blue."
>
> "I am allergic to peanuts."
>
> "My sister's birthday is March 15th."

**Overwrite Rule:** Explicit statements **overwrite** existing facts on the same subject-predicate.
- Old fact: `devansh favourite_colour Tuesday` (confidence 0.60, inferred)
- New fact: `devansh favourite_colour Monday` (confidence 1.00, explicit)
- Result: Old fact replaced. No ambiguity.

**Confidence:** 1.00 (user_edit source)

**Sensitive Detection:** If the statement contains health, medical, allergy, or safety information, the agent must explicitly confirm:
```
> "I am allergic to peanuts."

⚠️ I have learned: You have a **PEANUT ALLERGY**.
This is a sensitive health fact. Please confirm this is correct.

[Yes, that is right] [No, I made a mistake] [Edit the fact]
```

### 2. Casual Mention (Low Priority, No Overwrite)
The user mentions something incidentally, not as a direct assertion.

**Trigger:** Mention in passing during conversation.
> "I like Mondays because the office is quiet."
>
> "Blue is a nice colour."

**Overwrite Rule:** Casual mentions **do NOT overwrite** existing facts.
- Existing fact: `devansh favourite_colour green` (confidence 0.80, explicit)
- Casual mention: "Blue is a nice colour" → Creates fact with low confidence (0.30)
- Result: Both facts coexist. The explicit one remains primary.

**Confidence:** 0.20–0.40 (inferred from conversation)

**Why:** The user may be expressing a passing thought, not a definitive preference. Overwriting explicit facts from casual mentions would be fragile.

### 3. Connector Extraction (Medium Priority)
Facts extracted from external services (email, calendar, photos).

**Overwrite Rule:** Connector facts **do not overwrite explicit user facts**, but they **overwrite inferred facts**.
- Existing explicit: `devansh lives_in London`
- Connector infers: `devansh lives_in Manchester` → Rejected (conflicts with explicit)
- Existing inferred: `devansh works_as Engineer` (confidence 0.50)
- Connector infers: `devansh works_as Engineer` (confidence 0.90, from LinkedIn) → Overwrites inferred fact

**Confidence:** Depends on extraction method and source reliability.

### 4. Reasoning Inference (Lowest Priority)
Facts derived by the Reasoning Engine.

**Overwrite Rule:** Inferred facts **never overwrite** any other fact type. They coexist with lower confidence.
- Existing: `devansh visited Rome` (confidence 0.95, from photos)
- Inference: `devansh visited Colosseum` (confidence 0.60, derived from "Roman History Tour" email)
- Result: Both facts stored. If contradiction found, inference flagged for review.

**Confidence:** 0.30–0.60 typically.

## Explicit vs. Casual Detection

The agent uses an LLM-based classifier to determine if a user utterance is explicit or casual:

```
Input: "My favourite day is Monday."
Classifier: EXPLICIT — Direct assertion of preference. Subject + predicate + object clearly stated.
Action: Store with confidence 1.00. Overwrite existing favourite_day.

Input: "I like Mondays because the office is quiet."
Classifier: CASUAL — Expresses enjoyment but not a definitive preference.
Action: Store with confidence 0.30. Do not overwrite.

Input: "Monday is my favourite day, not Tuesday."
Classifier: EXPLICIT + CORRECTION — Direct assertion with contradiction resolution.
Action: Update favourite_day to Monday. Delete or lower confidence of Tuesday fact.
```

### Classification Features
- Presence of possessive language ("my favourite", "I prefer", "I am")
- Definitive vs. hedging language ("is" vs. "is kind of nice")
- Correction markers ("not", "actually", "wait")
- Topic focus (entire utterance about the fact vs. passing mention)

## Sensitivity Detection and Confirmation

### What Counts as Sensitive?
- Health conditions, allergies, medications
- Financial details (salary, debts, investments)
- Relationship status and family details
- Religious or political beliefs
- Legal status (citizenship, visas)

### Confirmation Flow
When a sensitive fact is detected:
1. Store the fact with `pending_confirmation: true`
2. Present explicit confirmation to user
3. If confirmed: `confidence = 1.00`, `pending_confirmation = false`
4. If rejected: Delete the fact, log rejection
5. If ignored: Fact remains pending for 7 days, then auto-deleted

### Persistent Sensitive Facts
Once confirmed, sensitive facts are treated as high-confidence and protected:
- Never mentioned in public/shared contexts
- Never used for inference without explicit permission
- Exported with clear sensitivity markers in Obsidian format
- Require explicit confirmation before bulk deletion

## Fact Overwrite Rules Summary

| New Fact ↓ / Existing → | Explicit (1.0) | Connector (0.7–0.9) | Casual (0.2–0.4) | Inference (0.3–0.6) |
|------------------------|----------------|---------------------|------------------|---------------------|
| **Explicit (1.0)** | Overwrite | Overwrite | Overwrite | Overwrite |
| **Connector (0.7–0.9)** | Coexist | Overwrite | Overwrite | Overwrite |
| **Casual (0.2–0.4)** | Coexist | Coexist | Coexist | Coexist |
| **Inference (0.3–0.6)** | Coexist | Coexist | Coexist | Coexist |

**Coexist:** Both facts stored. Higher confidence fact presented as primary. **Overwrite:** New fact replaces old fact. Old fact soft-deleted with reason.

## Teaching Examples

### Direct Teaching (Chat)
```
> "I am allergic to peanuts and shellfish."

⚠️ I have learned two sensitive health facts:
1. You have a PEANUT ALLERGY.
2. You have a SHELLFISH ALLERGY.

These are important safety facts. Please confirm they are correct.

[Confirm all] [Edit] [Reject all]
```

### Correction via Explicit Re-assertion
```
> "You said my favourite colour was blue, but it is actually green."

Noted. I have updated your favourite colour from blue to green.
The previous fact has been replaced.
```

### Casual Mention (No Overwrite)
```
> "Green is a calming colour."

I will note that you appreciate green, but I will not overwrite your stated favourite colour (which is blue).
```

## Bulk Learning

The agent supports bulk import of facts from files, Obsidian vaults, and spreadsheets. See `Bulk-Import.md`.
