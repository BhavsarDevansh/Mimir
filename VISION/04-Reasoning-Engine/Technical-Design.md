# Reasoning Engine — Technical Design

## Philosophy
Every query is an investigation. The agent starts with a direct lookup. If the answer is not a single, high-confidence fact, it launches a multi-threaded investigation across all available sources. The user watches the investigation unfold in real time.

## Investigation Model

### Every Query Is an Investigation
```
User: "When is my next flight?"
  → Direct tool: query calendar
  → Result: "JL043 to Tokyo, May 25, 11:00 AM"
  → Investigation complete. Answer returned.

User: "When was I last in Rome?"
  → Direct KB lookup: found "visited Rome 2022" and "visited Rome 2025"
  → Ambiguous. Launch multi-thread investigation.
  → Thread 1: Photos (GPS near Rome)
  → Thread 2: Calendar events (Rome trips)
  → Thread 3: Emails (Rome references)
  → Thread 4: Messages (Rome mentions)
  → Thread 5: External (web search Roman History Tour)
  → Synthesis: May 2025 is most recent, corroborated by 4 sources
```

### Configurable Thread Limits
```rust
struct InvestigationConfig {
    max_threads: u32,       // Default: 5
    max_depth_per_thread: u32,  // Default: 3 (how many sub-questions per thread)
    max_web_searches: u32,  // Default: 5 per thread
    confidence_threshold: f32,  // Default: 0.90
    time_budget_seconds: u32,   // Default: 30
}
```

The user can configure these globally or per-query:
```bash
$ agent ask "What caused WWI?" --threads 8 --depth 5
```

## Core Structures

### Investigation
```rust
struct Investigation {
    id: u32,
    original_query: String,
    query_embedding: Vec<f32>,
    status: InvestigationStatus,  // Planning | Running | Evaluating | Complete | Failed
    threads: Vec<InvestigationThread>,
    meta_thread: Option<MetaThread>,  // Spawned if threads conflict
    config: InvestigationConfig,
    final_answer: Option<Answer>,
    created_at: DateTime,
    completed_at: Option<DateTime>,
}
```

### Investigation Thread
A single avenue of investigation that can drill down deeply.

```rust
struct InvestigationThread {
    id: u32,
    investigation_id: u32,
    name: String,             // e.g., "Photo Evidence", "Calendar Events"
    status: ThreadStatus,     // Running | Paused | Complete | Failed
    query: String,            // What this thread is investigating
    parent_thread: Option<u32>, // For sub-threads spawned from drilling down
    depth: u32,               // How deep in the drill-down chain
    evidence: Vec<Evidence>,
    hypothesis: Option<Hypothesis>,
    confidence: f32,
    sub_threads: Vec<u32>, // IDs of child threads
}
```

### Meta Thread
Spawned when multiple threads produce contradictory results.

```rust
struct MetaThread {
    id: u32,
    investigation_id: u32,
    conflicting_threads: Vec<u32>, // Thread IDs that contradict
    conflict_description: String,
    resolution_attempts: Vec<ResolutionAttempt>,
    status: MetaThreadStatus, // Running | Resolved | Unresolved
    final_assessment: Option<String>,
}
```

### Evidence
```rust
struct Evidence {
    id: u32,
    thread_id: u32,
    source: EvidenceSource,   // KnowledgeGraph | Connector | ExternalWeb | Inference
    source_reference: String,
    content: String,
    raw_data: serde_json::Value,
    reliability: f32,
    relevance: f32,
    extracted_facts: Vec<ExtractedFact>,
    timestamp: DateTime,
}
```

### Hypothesis
```rust
struct Hypothesis {
    id: u32,
    thread_id: u32,
    statement: String,
    confidence: f32,
    status: HypothesisStatus, // Unverified | Supported | Contradicted | Inconclusive
    supporting_evidence: Vec<u32>,
    contradicting_evidence: Vec<u32>,
}
```

## The Investigation Lifecycle

### Phase 1: Direct Lookup
```
1. Parse query intent
2. Check if a specific tool answers this directly
   - "Next calendar event" → Calendar connector
   - "Current time" → System tool
   - "Multiply 5 by 7" → Calculator tool
3. If tool returns single high-confidence result → Answer immediately
4. If ambiguous, incomplete, or no direct tool → Proceed to Phase 2
```

### Phase 2: Thread Planning
```
5. Generate initial hypotheses (LLM)
6. Plan investigation threads:
   - What sources are relevant? (KB, connectors, web)
   - What is the best order to check them?
   - What sub-questions might each thread uncover?
7. Spawn threads up to max_threads limit
8. Begin execution in parallel
```

### Phase 3: Thread Execution (Parallel)
```
9. Each thread executes independently:
   a. Query its assigned source
   b. Extract evidence
   c. Evaluate hypothesis confidence
   d. If evidence suggests new sub-questions:
      - Spawn sub-thread (if depth < max_depth)
      - e.g., "Roman History Tour" → sub-thread: "Does this tour include Colosseum?"
   e. Report findings back to main investigation
```

### Phase 4: Conflict Detection
```
10. Compare thread results
11. If threads agree (within confidence threshold) → Proceed to synthesis
12. If threads contradict:
    a. Spawn Meta Thread
    b. Meta thread investigates WHY the contradiction exists
    c. Attempt to reconcile (e.g., one source is outdated, one is misinterpreted)
    d. If reconciled → Unified answer
    e. If irreconcilable → Report conflict explicitly
```

### Phase 5: Synthesis
```
13. Select best-supported hypothesis
14. Construct answer with evidence trail
15. Flag any remaining uncertainties or conflicts
16. Stream final answer to user
```

### Phase 6: Learning
```
17. Store investigation result in KB
18. Update entity relationships
19. Log for pattern learning
```

## Thread Examples

### Query: "When was I last in Rome?"

**Thread 1: Photos**
- Query: Find photos with GPS near Rome
- Finds: IMG_2045.jpg (GPS 41.8902, 12.4924, 2025-05-05)
- Sub-thread: "Is this GPS definitely Rome, not nearby town?"
- Confidence: 0.98

**Thread 2: Calendar**
- Query: Find events in Rome
- Finds: "Trip to Rome" (2025-05-03 to 2025-05-07)
- Confidence: 0.99

**Thread 3: Emails**
- Query: Find emails about Rome
- Finds: "Rate your Roman History Tour" (2025-05-08)
- Sub-thread: "Does Roman History Tour include Rome/Colosseum?"
- Web search confirms: Yes, includes Colosseum
- Confidence: 0.90 (inferred attendance from tour)

**Thread 4: Messages**
- Query: Find messages mentioning Rome
- Finds: Friend says "How was Rome?" (2025-05-10), User replies "Amazing!"
- Confidence: 0.85

**Thread 5: External (Web)**
- Query: Any social media posts or public references
- Finds: No public posts
- Confidence: N/A (no evidence)

**Meta Thread:** All threads agree on May 2025. No conflict. Meta thread not needed.

**Synthesis:** "You were last in Rome May 3–7, 2025. Confirmed by calendar, photos, and messages."

### Query: "What led to the fall of the Roman Republic?"

**Thread 1: Gracchi Brothers**
- Query: Tiberius and Gaius Gracchi reforms
- Finds: Land reforms, political violence escalation
- Sub-thread: "Did Gracchi reforms directly cause fall, or were they early warning?"
- Confidence: 0.80

**Thread 2: Military Reforms**
- Query: Marius professional army, Sulla civil war
- Finds: Army loyalty shifts to generals
- Sub-thread: "Could Republic have survived without military reform?"
- Confidence: 0.85

**Thread 3: Julius Caesar**
- Query: Caesar's rise, Rubicon, assassination
- Finds: Dictatorship ends republican institutions
- Sub-thread: "Was Caesar inevitable given prior events?"
- Confidence: 0.90

**Thread 4: Augustus Principate**
- Query: Augustus transition to Empire
- Finds: Formal end of Republic in 27 BCE
- Sub-thread: "Did Augustus preserve or destroy Republic?"
- Confidence: 0.75

**Thread 5: Economic Factors**
- Query: Roman economic inequality, slave economy
- Finds: Wealth concentration, land crisis
- Confidence: 0.70

**Meta Thread:** Thread 2 and Thread 3 disagree on whether military reform or Caesar was the decisive factor. Meta thread investigates: "Was Caesar's crossing of the Rubicon a symptom of the military reform problem, or an independent cause?"
- Resolution: Meta thread concludes military reform enabled Caesar, but Caesar's individual choices were the proximate cause. Both contributed.

**Synthesis:** "The fall was a century-long process with multiple causes. The Gracchi exposed institutional failure, Marius's reforms militarized politics, and Caesar's dictatorship made the Republic unworkable. Augustus formalized what was already dead."

## Real-Time Streaming

The user sees investigation progress as it happens:

```
> "When was I last in Rome?"

🔍 Starting investigation...
Thread 1/5: Searching photos → Found: IMG_2045.jpg (Rome, May 2025)
Thread 2/5: Searching calendar → Found: "Trip to Rome" (May 3–7, 2025)
Thread 3/5: Searching emails → Found: Tour confirmation
  → Sub-thread: Checking tour details... → Includes Colosseum ✅
Thread 4/5: Searching messages → Found: "How was Rome?" "Amazing!"
Thread 5/5: Web search → No additional references found

✅ All threads corroborate May 2025.

Answer: You were last in Rome from May 3–7, 2025.
Sources: Photos, Calendar, Email, Messages (4 independent sources)
Confidence: 0.99
```

If a meta thread is spawned:
```
⚠️ Thread conflict detected:
  Thread 2 (Military Reform) vs. Thread 3 (Caesar's role)
  Spawning meta-thread to reconcile...
  
Meta-thread: Investigating causal relationship...
  → Military reform enabled Caesar's rise
  → Caesar's choices were decisive proximate cause
  → Both contributed; no single factor sufficient

Answer: The fall was multi-causal...
```

## Stopping Criteria

An investigation stops when:

1. **Direct hit:** A tool returns a single unambiguous answer
2. **Consensus:** All active threads agree within confidence_threshold
3. **Exhaustion:** All high-quality sources checked, no new evidence emerging
4. **Time budget:** Configurable timeout reached (return best-effort answer)
5. **User interrupt:** User says "that's enough" or "just give me the answer"

The agent reports which criterion triggered the stop:
```
"Investigation complete (consensus reached, 4/5 threads agree, confidence 0.95)."
"Investigation halted (time budget exceeded, best-effort answer below)."
```

## Confidence Scoring

Per-thread confidence:
```
confidence = base_confidence(hypothesis)
  × source_reliability(evidence)
  × corroboration_multiplier(num_independent_sources)
  × recency_boost(if temporal query)
  × consistency_check(no_internal_contradictions)
```

Overall investigation confidence:
```
overall = average(thread_confidences) × consensus_bonus
  
consensus_bonus = 1.2 if all threads agree
                  0.8 if threads contradict (even after meta-thread)
                  0.5 if meta-thread unresolved
```

## Caching and Learning

- Investigation results cached by query hash
- Common query patterns pre-warmed
- Failed investigations analyzed to improve future thread planning
- Meta-thread resolutions added to KB as causal knowledge

## Technology Stack
- **Threading:** tokio async tasks
- **Orchestration:** Custom investigation state machine
- **LLM Planning:** Structured output (JSON mode) for hypothesis and thread generation
- **Streaming:** SSE/WebSocket for real-time progress
- **Storage:** SQLite (shared with Knowledge Graph)
