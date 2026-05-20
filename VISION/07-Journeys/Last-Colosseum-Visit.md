# Journey: Last Colosseum Visit

## Trigger
User asks: "When was I last in the Colosseum?"

## Flow

### 1. Core Agent Receives Query
- Input Router classifies as "temporal + location" question
- Routes to Reasoning Engine with `max_depth = 3`, `min_sources = 2`

### 2. Reasoning Engine Plans Investigation

**Hypotheses Generated:**
1. I was at the Colosseum on a specific recent date
2. I was at the Colosseum during a trip to Rome
3. I have never been to the Colosseum

### 3. Evidence Gathering

**Knowledge Graph Query:**
- Search entity "Colosseum" → found
- Query facts: `subject = devansh, predicate = visited, object = Colosseum`
- Results:
  - Fact A: `devansh visited Rome` (2022-04-10 to 2022-04-15, confidence 0.95)
  - Fact B: `devansh visited Rome` (2025-05-03 to 2025-05-07, confidence 0.99)

**Connector Queries:**

*Photos Connector:*
- Search GPS near Colosseum coordinates (41.8902, 12.4924)
- Found:
  - Photo IMG_1042.jpg (2022-04-12, GPS match, confidence 0.98)
  - Photo IMG_2045.jpg (2025-05-05, GPS match, confidence 0.98)

*Calendar Connector:*
- Search "Rome OR Colosseum OR Roman"
- Found:
  - Event: "Trip to Rome" (2025-05-03 to 2025-05-07)

*Email Connector:*
- Search "Colosseum OR Roman Forum OR tour"
- Found:
  - Email: "Rate your Roman History Tour" (2025-05-08, from tourcompany@roma.com)

### 4. Hypothesis Evaluation

**Hypothesis 1: Colosseum visit in 2022**
- Supporting: Photos with GPS at Colosseum (2022-04-12)
- Confidence: 0.99
- Status: **Supported**

**Hypothesis 2: Colosseum visit in 2025**
- Supporting: Photos with GPS at Colosseum (2025-05-05)
- Supporting: Calendar event "Trip to Rome" overlapping
- Weak supporting: Email about "Roman History Tour" after trip
- Missing: Does tour include Colosseum?
- Confidence: 0.75 (tour correlation unclear)
- Status: **Unverified**

### 5. Deep Investigation (Depth 2)

Reasoning Engine notes Hypothesis 2 lacks confirmation about tour contents.
- **External Research:** Web search "Roman History Tour Roma includes Colosseum"
- **Result:** Tour company website lists "Colosseum, Roman Forum, Palatine Hill"
- **Confidence raised to 0.99**

### 6. Synthesis

**Answer construction:**
- Most recent visit: May 5, 2025 (during Rome trip)
- Prior visit: April 12, 2022
- Sources: 2 photos (GPS-verified), 1 calendar event, 1 email, 1 web source
- Overall confidence: 0.99

### 7. Response to User

**Normal mode:**
> You were last at the Colosseum on May 5, 2025, during your trip to Rome.

**Verbose mode:**
> You were last at the Colosseum on **May 5, 2025**.
> 
> I found:
> - A photo taken at the Colosseum on that date
> - You were in Rome from May 3–7
> - You took a "Roman History Tour" that included the Colosseum
> 
> You were also there on April 12, 2022.

### 8. Knowledge Graph Update
- New fact: `devansh visited Colosseum` (2025-05-05, confidence 0.99)
- New fact: `devansh visited Colosseum` (2022-04-12, confidence 0.99)
- Investigation logged for learning
