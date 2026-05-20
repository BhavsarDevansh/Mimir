# Reasoning Engine — User Experience

## The Investigation Metaphor

When you ask a complex question, the agent behaves like a journalist or detective: it forms hypotheses, gathers evidence across multiple sources, evaluates confidence, and synthesizes an answer. You watch the investigation unfold in real time.

## Investigation Modes

### Normal Mode: Streaming Investigation
By default, you see the investigation as it happens:

```
> "When was I last in the Colosseum?"

🔍 Starting investigation (5 threads)...
Thread 1/5: Searching photos... → Found: IMG_2045.jpg (Rome, May 2025) ✅
Thread 2/5: Searching calendar... → Found: "Trip to Rome" (May 3–7) ✅
Thread 3/5: Searching emails... → Found: Tour confirmation
  → Checking: Does tour include Colosseum? → Yes ✅
Thread 4/5: Searching messages... → Found: "How was Rome?" "Amazing!" ✅
Thread 5/5: Web search... → No additional references

✅ Consensus reached (4/5 threads agree)

You were last at the Colosseum on **May 5, 2025**.
You were also there in 2022.
Confidence: 0.99 (4 independent sources)
```

### Verbose Mode: Full Transparency
With `--verbose`, you see every hypothesis, evidence item, and confidence update:

```
> "When was I last in the Colosseum?" --verbose

🔍 Starting investigation...

**Phase 1: Planning**
Generated hypotheses:
1. You were there on a specific recent date
2. You were there during a trip to Rome (range answer)
3. You have never been there

Planned 5 investigation threads:
- Thread 1: Photo evidence (GPS matching)
- Thread 2: Calendar events (Rome trips)
- Thread 3: Email references (tours, bookings)
- Thread 4: Message history (mentions of Rome)
- Thread 5: External references (web search)

**Phase 2: Evidence Gathering**

Thread 1 (Photos):
  - Query: GPS near Colosseum coordinates
  - Found: IMG_1042.jpg (2022-04-12, GPS match, confidence 0.98)
  - Found: IMG_2045.jpg (2025-05-05, GPS match, confidence 0.98)
  - Sub-thread: Verify GPS → Confirmed Rome city boundary
  - Thread confidence: 0.98

Thread 2 (Calendar):
  - Query: Events in Rome
  - Found: "Trip to Rome" (2025-05-03 to 2025-05-07, confidence 0.99)
  - Thread confidence: 0.99

Thread 3 (Emails):
  - Query: Colosseum OR Roman Forum OR tour
  - Found: "Rate your Roman History Tour" (2025-05-08)
  - Sub-thread: Does tour include Colosseum?
    - Web search: "Roman History Tour includes Colosseum, Roman Forum, Palatine Hill"
    - Confidence: 0.90 (inferred attendance)
  - Thread confidence: 0.90

Thread 4 (Messages):
  - Query: Rome mentions
  - Found: Friend: "How was Rome?" → You: "Amazing!" (2025-05-10)
  - Thread confidence: 0.85

Thread 5 (Web):
  - Query: public references to user in Rome
  - Found: None
  - Thread confidence: N/A

**Phase 3: Conflict Detection**
No conflicts detected. All threads point to May 2025.

**Phase 4: Synthesis**
Selected hypothesis: "You were at the Colosseum in 2025"
Supporting evidence: 4 independent sources
Confidence: 0.99

Answer: You were last at the Colosseum on **May 5, 2025**, during your trip to Rome.
```

### Example: Meta-Thread Conflict Resolution

```
> "What led to the fall of the Roman Republic?"

🔍 Starting investigation (5 threads)...
Thread 1: Gracchi Brothers reforms → Found: land reform, political violence ✅
Thread 2: Military reforms (Marius, Sulla) → Found: army loyalty shifts ✅
Thread 3: Julius Caesar → Found: Rubicon, dictatorship ✅
Thread 4: Augustus Principate → Found: formal end in 27 BCE ✅
Thread 5: Economic factors → Found: wealth inequality, slave economy ✅

⚠️ Thread conflict detected:
  Thread 2 argues military reform was root cause
  Thread 3 argues Caesar's actions were decisive

🔍 Spawning meta-thread to reconcile...
Meta-thread: Investigating causal chain...
  → Military reform enabled generals to challenge state (Thread 2)
  → Caesar exploited this to seize power (Thread 3)
  → Both necessary; neither alone sufficient

✅ Conflict resolved: Multi-causal explanation

The fall of the Roman Republic was a century-long unraveling:
- **Gracchi Brothers (133 BCE):** Exposed institutional failure
- **Military Reforms (107 BCE):** Made armies loyal to generals, not Rome
- **Civil Wars (88–31 BCE):** Destroyed republican institutions
- **Caesar (49 BCE):** Crossed Rubicon; made dictatorship inevitable
- **Augustus (27 BCE):** Formalized the Empire

The Republic fell because its institutions could not adapt to imperial scale,
economic stratification, and military strongmen.
```

## User Controls

```bash
# Limit investigation threads
$ agent ask "What caused WWI?" --threads 8

# Limit depth per thread
$ agent ask "Why did Rome fall?" --depth 5

# Require minimum sources
$ agent ask "When was I in Tokyo?" --sources 3

# Quick mode (fewer threads, less depth)
$ agent ask "Next calendar event?" --quick

# Time budget (seconds)
$ agent ask "Explain quantum computing" --timeout 20
```

## Interrupting Investigations

You can stop or redirect an investigation mid-stream:

```
> "When was I last in Rome?"
🔍 Thread 1: Searching photos...

> "Actually, just check my calendar."
🛑 Pausing investigation...
📅 Calendar: "Trip to Rome" May 3–7, 2025
✅ Investigation complete (user-directed, 1 thread used)
```

## Audit Trail

Every investigation is logged:

```bash
$ agent audit last-question
Investigation: "When was I last in the Colosseum?"
Status: Complete (consensus)
Duration: 3.2 seconds
Threads: 5 launched, 5 completed, 0 failed
Meta-thread: None needed

Thread results:
  1. Photos → 2 matches, confidence 0.98
  2. Calendar → 1 match, confidence 0.99
  3. Emails → 1 match + web corroboration, confidence 0.90
  4. Messages → 1 match, confidence 0.85
  5. Web → No matches, confidence N/A

Final answer: "May 5, 2025"
Sources cited: 4
Confidence: 0.99
```

```bash
$ agent audit "Roman Republic"
Investigation: "What led to the fall of the Roman Republic?"
Status: Complete (meta-thread resolved conflict)
Duration: 12.8 seconds
Threads: 5 launched, 5 completed
Meta-thread: 1 spawned, conflict resolved

Thread results:
  1. Gracchi → Found: early institutional failure
  2. Military → Found: army loyalty shift (KEY CAUSE)
  3. Caesar → Found: proximate cause (KEY CAUSE)
  4. Augustus → Found: formalization
  5. Economic → Found: contributing factor

Meta-thread resolution:
  - Military reform enabled Caesar's rise
  - Caesar's choices were decisive
  - Both necessary; multi-causal explanation

Final answer: "Century-long multi-causal process..."
Confidence: 0.90
```
