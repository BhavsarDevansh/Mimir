# Temporal Facts and Knowledge Lifecycle

## Philosophy
Facts exist in time. "I live in London" is not eternally true — it is true for a bounded period. The Knowledge Graph preserves history. It does not delete facts when they become outdated; it adds new facts with new temporal bounds. The agent understands that the most recent fact is the current one, but older facts remain valid for their era.

## Temporal Storage Model

Every fact carries temporal metadata:

```rust
struct Fact {
    id: String,
    subject_id: String,
    predicate: String,
    object_id: String,
    
    // Temporal bounds
    valid_from: Option<DateTime>,   // When this became true
    valid_until: Option<DateTime>,  // When this stopped being true
    
    // Knowledge timestamps
    learned_at: DateTime,             // When we first recorded this
    updated_at: DateTime,             // When we last modified this
    
    confidence: f32,
    sources: Vec<Source>,
    
    // Status
    status: FactStatus,  // Active | Superseded | Corrected | Disputed
}
```

## Example: Living Location Over Time

```
Fact A: devansh lives_in Mumbai
  valid_from: 2015-06-01
  valid_until: 2023-01-15
  status: Superseded
  confidence: 0.95

Fact B: devansh lives_in London
  valid_from: 2023-01-16
  valid_until: 2026-03-01
  status: Superseded
  confidence: 0.98

Fact C: devansh lives_in Berlin
  valid_from: 2026-03-02
  valid_until: null  // Currently true
  status: Active
  confidence: 0.95
```

When asked "Where does devansh live?", the agent returns **Fact C** (most recent, active).
When asked "Where did devansh live in 2021?", the agent returns **Fact A**.
When asked "Has devansh ever lived in London?", the agent returns **Fact B**.

## Fact Statuses

### Active
The fact is currently believed to be true. No newer fact has superseded it.

### Superseded
A newer fact on the same subject-predicate has been added. The old fact remains valid for its temporal bounds but is no longer the "current" truth.

### Corrected
The fact was retroactively found to be wrong and has been replaced by a corrected version. The original fact is preserved with a `correction` link to the new fact.

```
Fact X: devansh visited Rome 2022-04-12
  status: Corrected
  correction: Fact Y
  reason: "User confirmed actual date was 2021-04-15"

Fact Y: devansh visited Rome 2021-04-15
  status: Active
  supersedes: Fact X
```

### Disputed
Multiple facts exist for the same subject-predicate with overlapping or conflicting temporal bounds. The agent marks them as disputed and flags for user resolution.

```
Fact P: devansh visited Rome 2022-04-12 (from photos, GPS)
Fact Q: devansh visited Rome 2021-04-15 (from user correction)
  status: Disputed
  reason: "Contradictory dates for same visit"
```

## Superseding Rules

When a new fact is added that covers the same subject-predicate:

1. If temporal bounds do not overlap: Both facts coexist, no superseding needed.
2. If temporal bounds overlap: New fact wins for overlapping period.
3. If new fact has no temporal bounds (timeless): New fact supersedes older one.

```
Old: devansh works_at Company A (2020-01-01 to 2023-06-30)
New: devansh works_at Company B (2023-07-01 to present)
Result: Old marked Superseded, New marked Active

Old: devansh works_at Company A (2020-01-01 to present)
New: devansh works_at Company B (2023-07-01 to present)
Result: Old valid_until updated to 2023-06-30, marked Superseded. New Active.
```

## Retroactive Corrections

When the agent discovers an existing fact is wrong (wrong date, wrong location, wrong person), it does not just delete it — it creates a corrected fact and links them.

### Correction Flow

1. **Detect Discrepancy**
   - New evidence contradicts existing fact
   - User explicitly corrects a fact
   - Cross-source corroboration reveals error

2. **Create Corrected Fact**
   - New fact with corrected values
   - Status: Active
   - Links to old fact via `supersedes`

3. **Update Old Fact**
   - Status changed to Corrected
   - Add `correction` link to new fact
   - Add reason for correction in audit log

4. **Cascade Evaluation**
   - Find all facts inferred FROM the old fact
   - Re-evaluate each inference with the corrected fact
   - If inference now fails, mark as Disputed or delete
   - If inference still holds, update source chain

### Example: Retroactive Date Correction

```
Old Fact: devansh visited Rome 2022-04-12
  Sources: Photo IMG_1042.jpg (GPS: Rome, date: 2022-04-12)
  Inferred downstream:
    - Fact A: "devansh visited Colosseum 2022-04-12" (inferred from tour email)
    - Fact B: "devansh took 12 photos in Rome" (counted from photo batch)

User corrects: "That photo was from 2021, not 2022."

New Fact: devansh visited Rome 2021-04-15
  status: Active
  supersedes: Old Fact
  sources: Photo IMG_1042.jpg (corrected date), user_explicit

Old Fact: devansh visited Rome 2022-04-12
  status: Corrected
  correction: New Fact

Downstream re-evaluation:
- Fact A: Tour email was actually 2021-04-10. Inference still holds! 
  Update Fact A: "devansh visited Colosseum 2021-04-15" ✅
- Fact B: Photo batch dated 2022-04-12 was actually 2021-04-15. 
  Fact B still valid, just date shifted. ✅
```

## Knowledge Base as Index, Not Mirror

The Knowledge Graph does not need to store every raw email, photo, or message. It stores **derived facts** with **pointers to sources**.

### What Is Stored in KB
- Structured facts: `devansh visited Rome 2022-04-12`
- Relationships: `Rome is_in Italy`
- Preferences: `devansh prefers aisle_seats`
- Inferred patterns: `devansh packs_before_long_flights`

### What Is NOT Stored in KB (but referenced)
- Full email text (KB stores: sender, subject, extracted entities, confidence)
- Raw photo pixels (KB stores: GPS, timestamp, detected objects, file path)
- Message contents (KB stores: extracted facts, sentiment, participants)

### Source References
Every fact links back to its raw source:
```
Fact: devansh visited Rome 2022-04-12
  Source 1: photo:IMG_1042.jpg (GPS 41.8902,12.4924, 2022-04-12)
  Source 2: email:ticket_roma_2022.pdf (booking confirmation, 2022-03-01)
```

The agent can always "go back to the source" if a fact is questioned.

### Lazy Re-Verification
If a fact's confidence is questioned, the agent can:
1. Re-query the Knowledge Graph
2. Re-examine the raw source (if still accessible)
3. Re-run extraction on the raw source
4. Compare with new evidence

This means the KB stays lightweight while maintaining traceability.

## Fact Refresh and Re-Verification

The agent periodically re-verifies high-importance facts:

### Auto-Refresh Candidates
- Employment (check GitHub, LinkedIn every 3 months)
- Residence (check calendar, photos for address signals)
- Relationships (check messages, calendar for contact frequency)
- Preferences (re-confirm if pattern changes)

### Refresh Flow
```
1. Identify candidate facts (last verified > 3 months ago)
2. Query connectors for new evidence
3. If new evidence contradicts existing fact: flag for correction
4. If new evidence supports existing fact: update `verified_at` timestamp
5. If no new evidence: gradually decay confidence (optional)
```

### User Control
```bash
# See facts pending refresh
$ agent kb refresh --pending
3 facts need re-verification:
  - devansh works_at Company Y (last verified: 2024-12)
  - devansh lives_in Berlin (last verified: 2025-01)
  - devansh has_partner Alice (last verified: 2024-11)

[Verify all] [Verify individually] [Skip]
```

```bash
# Disable auto-refresh for specific predicates
$ agent kb refresh --disable works_at
Disabled auto-refresh for employment facts.
```

## Confidence Decay

Optional: facts that have not been re-verified gradually lose confidence.

```
Confidence decay formula:
  new_confidence = base_confidence × decay_factor^(months_since_verified)
  
  decay_factor = 0.98 per month (configurable)
  
Example:
  Fact: devansh works_at Company Y, confidence = 0.95
  After 6 months without verification:
    confidence = 0.95 × 0.98^6 = 0.95 × 0.885 = 0.84
  After 12 months:
    confidence = 0.95 × 0.98^12 = 0.95 × 0.784 = 0.74
```

**User control:** Disable decay entirely, or set per-predicate decay rates.

## Archival

Facts with very low confidence or very old temporal bounds can be archived:
- Archived facts remain queryable but are not returned in default searches
- They appear in `kb query --include-archived`
- Useful for long-term pattern analysis without cluttering active queries
