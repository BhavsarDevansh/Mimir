# Nightly Knowledge Base Optimization

## Philosophy
The Knowledge Graph should maintain itself. Every night, the agent runs a background optimization that cleans, deduplicates, resolves contradictions, and consolidates facts — all without user intervention. This is "seamless intelligence."

## Optimization Schedule

### Every Night (02:00 local time)
```
02:00 — Start optimization
02:05 — Deduplication pass
02:15 — Contradiction scan
02:30 — Inference chain validation
02:45 — Performance compaction
02:55 — Confidence recalibration
03:00 — Optimization complete
```

### Weekly (Sunday 03:00)
- Full graph integrity check
- Pattern re-evaluation
- Entity resolution review
- Archive old low-confidence facts

### Monthly (First Sunday)
- Knowledge graph size analysis
- Index rebuild
- Full backup
- Suggest connector re-syncs if needed

## Optimization Passes

### 1. Deduplication
Find and merge duplicate facts.

**Detection criteria:**
- Same subject + predicate + object + overlapping temporal bounds
- Semantic duplicates (e.g., "lives_in London" and "lives_in London, UK")
- Near-duplicate sources (same email extracted twice with minor differences)

**Merge strategy:**
```
Duplicate pair:
  Fact A: devansh lives_in London (confidence 0.90, source: calendar)
  Fact B: devansh lives_in London (confidence 0.85, source: email)

Result:
  Merged fact: devansh lives_in London
    confidence: 0.95 (boosted from corroboration)
    sources: [calendar, email]
    merged_from: [Fact A, Fact B]
```

### 2. Contradiction Resolution
Find facts that cannot both be true.

**Contradiction types:**
- **Temporal overlap:** "visited Rome 2022-04" AND "visited Tokyo 2022-04-12" (possible but unlikely)
- **Mutual exclusion:** "lives_in London" AND "lives_in Berlin" at same time
- **Inferred vs. explicit:** Inferred fact contradicts explicit user statement
- **Source conflict:** Two connectors disagree on same event

**Resolution strategies:**

**A. Automatic (low-stakes):**
```
Contradiction: "devansh visited Rome 2022-04-12" vs. "2021-04-15"
Evidence: User explicitly corrected to 2021
Action: Mark 2022 as Corrected, 2021 as Active. No user intervention.
```

**B. Confidence-based (medium-stakes):**
```
Contradiction: "devansh works_at Company Y" vs. "devansh works_at Company Z"
Evidence: LinkedIn says Company Z, but no explicit user statement
Action: Reduce confidence of both. Flag for user review in next audit.
```

**C. Flag for user (high-stakes):**
```
Contradiction: "devansh has_partner Alice" vs. "devansh has_partner Bob"
Evidence: Both from different email threads, ambiguous
Action: Mark as Disputed. Notify user: "I found conflicting information about your partner. Can you clarify?"
```

### 3. Inference Chain Validation
Re-evaluate all inferred facts to ensure their source chain is still valid.

```
Inferred fact: "devansh visited Colosseum 2022-04-12"
  Derived from: "devansh visited Rome 2022-04-12"
  
Source fact updated: "devansh visited Rome 2021-04-15" (Corrected)

Action:
  - Re-evaluate inference: Is Colosseum visit still valid?
  - If new source does not support inference: Mark inferred fact as Disputed
  - If inference still holds with corrected date: Update inferred fact date
```

### 4. Confidence Recalibration
Adjust confidence scores based on:
- Time since last verification (decay)
- Number of corroborating sources
- Historical accuracy of source type
- User correction history

```
Before: "devansh favourite_colour blue" confidence = 0.95
  Last verified: 8 months ago
  No corroboration since then
  
After decay: confidence = 0.82
Action: No user notification. Fact still retrievable but lower priority.
```

### 5. Dormant Incorrect Fact Cleanup
Facts that are incorrect but were never explicitly deleted.

**Detection:**
- Fact has confidence < 0.30 and no recent corroboration
- Fact is marked Disputed for > 30 days with no user resolution
- Fact contradicts a high-confidence explicit fact
- Inference chain broken (source fact deleted or corrected)

**Action:**
```
Fact: "devansh visited Paris 2023-06" (confidence 0.25, single source, disputed)
  Source email was actually about a friend named Devansh, not this user.
  No corroboration found.
  
Action: Soft-delete (move to trash) with reason:
  "Dormant fact with no corroboration, likely false positive."
  
No user notification (seamless).
Fact recoverable from trash for 30 days.
```

### 6. Performance Compaction
Optimize the database for query performance.

```
- Rebuild FTS5 index
- VACUUM SQLite database
- Rebuild entity_fts
- Compact inference chain references
- Update materialized views (if any)
```

### 7. Pattern Consolidation
Merge related patterns to reduce redundancy.

```
Pattern A: "Before long flights, user packs 4 hours before"
Pattern B: "Before flights > 8h, user packs evening before"

Consolidated: "Before flights, user packs based on duration:
  - < 4h: 2 hours before
  - 4–8h: 4 hours before
  - > 8h: evening before"
```

## Invisible by Design

The user never sees optimization happening unless they explicitly check:
```bash
$ agent kb optimization --status
Last optimization: 2025-05-20 02:00–03:00
Results:
  - Deduplicated: 45 facts → 18 merged
  - Contradictions resolved: 3 (2 automatic, 1 flagged for review)
  - Inference chains re-evaluated: 234 (12 updated, 2 disputed)
  - Confidence decay applied: 456 facts
  - Dormant facts removed: 7 (moved to trash)
  - Database compacted: 12MB → 8MB
  - Patterns consolidated: 3

Current health: ✅ Excellent
```

### If Something Needs User Attention
If optimization finds a high-stakes contradiction that cannot be resolved automatically:
```
🔔 Knowledge Review Needed

During last night's optimization, I found a contradiction I cannot resolve:

"devansh has_partner Alice" vs. "devansh has_partner Bob"

Can you clarify?
[Alice] [Bob] [Neither / Prefer not to say] [Review details]
```

## Resource Management

Optimization is resource-conscious:
- Runs at low priority (nice +10)
- Limited to 1 CPU core by default
- Can be paused/resumed if user activity detected
- Timeout: 2 hours. If not complete, resumes next night.

```bash
# User can disable or reschedule
$ agent config optimization.time 04:00
$ agent config optimization.disable false
```

## Recovery

If optimization corrupts data (extremely unlikely):
```bash
$ agent kb restore --from-backup --date 2025-05-19
Restored Knowledge Graph from pre-optimization backup.
```

Automatic backups are taken before every optimization.
