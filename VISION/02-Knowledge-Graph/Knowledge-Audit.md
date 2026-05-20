# Knowledge Audit

## Philosophy
The user should never feel subject to a black box. They must be able to inspect, browse, and challenge everything the agent knows. The Knowledge Audit system provides multiple lenses into the agent's model of the user.

## Audit Views

### 1. Knowledge Profile
A structured, human-readable biography assembled from the Knowledge Graph. This is what the agent "believes" about the user.

```bash
$ agent kb profile
Generating knowledge profile...

═══════════════════════════════════════════════════════════
                    Knowledge Profile
═══════════════════════════════════════════════════════════

**Identity**
Name: Devansh
Aliases: Dev
Pronouns: he/him
Birthday: March 15 (confidence: 0.95)

**Residence**
Current: Berlin, Germany (since March 2026)
Previous: London, UK (2023–2026)
           Mumbai, India (2015–2023)

**Employment**
Current: Software Engineer at Company Y (since July 2023)
Previous: Engineer at Company X (2020–2023)

**Travel**
Recent: Rome (May 2025), Tokyo (May 2025), Lisbon (2024)
Frequent destinations: London, Mumbai, Berlin
Airline preference: Japan Airlines (4 flights)
Seat preference: Aisle (confidence: 0.85)

**Relationships**
Partner: Alice (since 2021)
Sister: Priya
Nephew: Arjun (birthday: June 10)

**Preferences**
Colours: Blue > Green > Red
Music: Jazz, ambient electronic
Food: Indian, Japanese
Pizza: No pineapple (explicit rejection)

**Health**
Allergies: Peanuts, shellfish

**Habits**
Morning routine: Coffee at 8 AM, email check at 9 AM
Packing: Usually 4 hours before flights

═══════════════════════════════════════════════════════════
Sources: 234 facts | Confidence > 0.70: 198 facts
Last updated: 2025-05-20 14:30

[Edit fact] [Report error] [Export to Markdown]
```

The profile is auto-generated from the Knowledge Graph and updated daily. It is purely a view — editing it edits the underlying facts.

### 2. Category Browser
Browse facts by domain.

```bash
$ agent kb browse --category travel
Travel Facts (47):

Places Visited:
  Rome          2022-04, 2025-05    [photos, calendar, email]
  Tokyo         2025-05              [photos, calendar]
  Lisbon        2024-09              [photos]
  ...

Travel Preferences:
  Seat: Aisle (0.85)          [learned from 6 bookings]
  Airline: JAL (0.70)         [frequent flyer pattern]
  Packing: 4h before (0.90)   [calendar + home assistant]

Upcoming:
  Barcelona     2025-08 (calendar event)

[Add fact] [Edit] [Delete] [Browse another category]
```

Available categories:
- identity, residence, employment, travel, relationships, health, preferences, habits, finances, interests

### 3. Uncertainty Review
Surface facts the agent is unsure about or that might be wrong.

```bash
$ agent kb review --uncertain
Facts needing review (12):

🟡 Medium confidence (0.50–0.70):
  - devansh has_partner Alice (0.60, single source: mention in email)
  - devansh works_remote (0.55, inferred from home assistant patterns)

🔴 Contradictions:
  - devansh visited Rome 2022-04-12 (photo) vs. 2021-04-15 (user correction)
    [Resolve]

🟠 Single-source facts:
  - devansh likes jazz (0.65, single Spotify playlist)
  - devansh speaks German (0.50, inferred from Berlin residence)

[Review all] [Review individually] [Dismiss]
```

**Surprise Me Mode:**
The agent occasionally surfaces unexpected facts it has inferred:
```
🔍 Interesting Inference

I noticed something: You seem to listen to more jazz on rainy days.
This is a weak pattern (4 occurrences) but I found it curious.

[Interesting, keep tracking] [Probably coincidence, ignore] [Show evidence]
```

### 4. Source Audit
See exactly where every fact came from.

```bash
$ agent kb audit --entity devansh --predicate lives_in
Fact: devansh lives_in Berlin
  Learned: 2026-03-02
  Confidence: 0.95
  Sources:
    - calendar: "Move to Berlin" event (2026-03-01)
    - email: "Welcome to your new flat" from landlord (2026-03-02)
    - photo: IMG_3091.jpg GPS 52.5200,13.4050 (2026-03-05)
  Inferred from: 3 sources
  Never corrected

Fact: devansh lives_in London
  Learned: 2023-01-16
  Confidence: 0.98
  Sources:
    - calendar: "Move to London" event (2023-01-15)
    - email: Council tax bill (2023-01-20)
  Superseded by: "lives_in Berlin" (2026-03-02)
```

### 5. Confidence Heatmap
Visual overview of fact confidence by category.

```bash
$ agent kb heatmap
Confidence by category:

Identity      ████████████████████░░  94%
Residence     ████████████████████░░  96%
Employment    ██████████████████░░░░  88%
Travel        ████████████████░░░░░░  82%
Relationships ████████████░░░░░░░░░░  65%
Health        ████████████████░░░░░░  85%
Preferences   ██████████████░░░░░░░░  72%
```

Low-confidence categories suggest areas where the agent needs more evidence or the user has been less explicit.

## Natural Language Audit Queries

Users can audit via natural language:
```bash
> "What do you know about my travel?"
> "Show me everything you are unsure about."
> "How did you learn that I live in Berlin?"
> "What facts do you know that came from my emails?"
> "Surprise me with something you have inferred."
```

## Export and Backup

### Full Export
```bash
$ agent kb export --all ~/backups/agent-kb-2025-05-20.json
Exported 12,304 entities and 48,291 facts.
```

### Category Export
```bash
$ agent kb export --category health ~/health-facts.json
Exported 23 health-related facts.
```

### Obsidian Export
```bash
$ agent kb export --format obsidian ~/Obsidian/Agent-Knowledge/
```
