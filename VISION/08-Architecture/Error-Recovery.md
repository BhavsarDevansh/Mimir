# Error Recovery and Self-Correction

## Philosophy
Mistakes are inevitable. What separates a trusted agent from an annoying one is how it detects, admits, and learns from errors. The agent must be able to trace back through its reasoning chains, understand where it went wrong, and adjust not just the specific fact but the underlying patterns and confidence models that led to the error.

## Types of Errors

### 1. Extraction Errors
The agent misread raw data. Examples:
- Parsed a flight confirmation for a friend as your flight
- Misread a date format (MM/DD vs DD/MM)
- Misidentified a person in a photo

**Detection:** Often found when user corrects or deletes an auto-added fact.

### 2. Inference Errors
The agent drew a wrong conclusion from correct data. Examples:
- Assumed you attended a tour because you received the confirmation email
- Inferred a relationship between two people based on a single mention
- Connected two events as cause-and-effect incorrectly

**Detection:** Requires user correction or internal contradiction detection.

### 3. Action Errors
The agent did something the user did not want. Examples:
- Added a flight to calendar that was not yours
- Sent a proactive notification at a bad time
- Suggested an action the user explicitly avoids

**Detection:** User rejection, dismissals, or explicit correction.

### 4. Confidence Overestimation
The agent was more certain than it should have been. Examples:
- "I am 99% confident" when the evidence was actually ambiguous
- Presented a low-confidence inference as a high-confidence fact

**Detection:** Internal calibration checks and user feedback.

## Self-Correction Strategy

### Layer 1: Internal Contradiction Detection
Before presenting a fact or taking an action, the agent checks for internal contradictions.

```
Check: Does this new fact contradict any existing high-confidence fact?
Example: New fact "devansh has flight JL043 on May 25"
Existing fact: "devansh has flight JL044 on May 25 to Osaka"
Contradiction: Two flights on same day? Possible, but flag for review.
Action: Lower confidence, present as "possible conflict"
```

### Layer 2: Pattern Break Detection
If the agent detects a break in an established pattern, it pauses before acting.

```
Pattern: "User always adds flight emails to calendar within 1 hour"
Break: "Flight email received 3 hours ago, not added to calendar"
Action: "I noticed you usually add flight emails quickly, but you have not added this one. Want me to skip it?"
```

### Layer 3: User Correction Trigger
When the user corrects an error, the agent initiates a full chain re-evaluation.

## Chain Re-Evaluation

When a user says "That is wrong" or deletes a fact, the agent does not just remove the fact — it traces backwards.

### Step 1: Identify the Erroneous Fact
```
Fact: "devansh visited Colosseum 2025-05-05"
User says: "I cancelled that tour, I was not there"
Action: Mark fact as contradicted, confidence → 0.0
```

### Step 2: Trace Inference Chain
```
Fact derived from:
- Raw Event: Email "Rate your Roman History Tour" (2025-05-08)
- Extracted Fact: "devansh has_booking Roman History Tour" (confidence 0.90)
- Inferred Fact: "Roman History Tour includes Colosseum" (confidence 0.85, web source)
- Inferred Fact: "devansh visited Colosseum" (confidence 0.76, derived)
```

### Step 3: Evaluate Each Link
For each link in the chain, reassess confidence:
- **Email source:** Still valid, but might be for a friend or a cancelled booking
- **Extraction:** "has_booking" does not mean "attended" — this is a valid inference gap
- **Web source:** Tour does include Colosseum, but this does not prove attendance
- **Final inference:** Overstated confidence; conflated "booking exists" with "attended"

### Step 4: Update Derived Facts
```
"devansh has_booking Roman History Tour" → confidence reduced from 0.90 to 0.60
"devansh visited Colosseum" → deleted, with reason "user correction"
"Roman History Tour includes Colosseum" → confidence unchanged (factually correct)
```

### Step 5: Learn Pattern Adjustments
```
New rule learned:
- "has_booking" does NOT imply "attended" unless corroborated by photo, GPS, or explicit confirmation
- Reduce confidence of attendance inferences by 30% when only source is booking email
- Create pattern: "user cancelled tour" → attendance inference invalidated
```

### Step 6: Notify User of Learning
```
> "I was wrong. I assumed the tour booking meant you attended, but you cancelled. I have corrected my knowledge and I will be more careful about inferring attendance from bookings alone."
```

## Confidence Recalibration

When errors are detected, the agent recalibrates confidence scores for similar facts:

```rust
struct ConfidenceRecalibration {
    trigger_fact_id: String,
    error_type: ErrorType,
    affected_predicate: String,
    confidence_adjustment: f32,  // e.g., -0.30
    scope: RecalibrationScope,   // this_fact | similar_facts | all_facts_with_predicate
    reason: String,
}
```

Example: After discovering an attendance inference error, reduce confidence on all `visited` facts inferred solely from booking emails by 30%.

## User Correction Patterns

The agent learns from different types of corrections:

| User Action | Agent Response | Learning |
|-------------|---------------|----------|
| "That is not mine" | Delete fact, trace chain | Source attribution error; improve entity resolution |
| "I cancelled that" | Delete attendance, keep booking | Booking ≠ attendance; learn cancellation signal |
| "Not now" (dismissed proactive) | Log dismissal, no action | Reduce trigger confidence for this pattern |
| "Never do that" | Create negative permission | Category-level prohibition |
| Edits a fact | Accept edit, ask why | Specific correction → general rule |
| Deletes without comment | Mark as uncertain, wait for more evidence | Ambiguous correction; do not overcorrect |

## Recovery Gracefully

When the agent makes an action error (e.g., added wrong event to calendar):

1. **Acknowledge immediately**
   > "I made a mistake — that event was not yours. I am removing it."

2. **Undo if possible**
   - Delete the calendar event
   - Revert any state changes

3. **Explain without being defensive**
   > "I misidentified the email recipient. I thought it was your confirmation, but it was your friend's. I will check more carefully next time."

4. **Adjust confidence**
   - Reduce confidence of similar extractions by sender/domain

5. **Offer repair**
   > "Should I add a rule to double-check flight emails if the name does not match yours?"

## Preventing Cascading Errors

A single wrong fact can pollute downstream inferences. The agent implements cascade limits:

```
Max inference depth: 3 (configurable)
Confidence floor for inference: 0.60
Inferred facts must have 2+ independent sources to reach 0.90+
User corrections immediately invalidate all downstream inferred facts
```

## Periodic Self-Audit

The agent runs a weekly self-audit:
1. Identify facts with low confidence that have not been corroborated
2. Check for contradictions among high-confidence facts
3. Review recent user corrections for patterns
4. Recalibrate confidence models
5. Report to user: "This week I found 3 uncertain facts and 1 possible contradiction. Want to review?"
