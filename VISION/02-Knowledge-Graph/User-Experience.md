# Knowledge Graph — User Experience

## The Knowledge Base as External Memory

The Knowledge Graph is the agent's persistent memory of everything it knows about you and the world. You can inspect it, edit it, and browse it.

### CLI Inspection
```bash
# Query facts about yourself
$ agent kb query "me"
Entity: devansh
  - type: Person
  - lives_in: London (confidence: 0.95, source: calendar, since: 2023-01)
  - works_as: Software Engineer (confidence: 0.88, source: linkedin, since: 2022-06)
  - visited: Rome (confidence: 0.99, source: photos+email+calendar, dates: [2022-04, 2025-05])

# Inspect a specific relationship
$ agent kb show "devansh visited Rome 2025-05"
Fact ID: f-7a3b9c
  Subject: devansh
  Predicate: visited
  Object: Rome
  Temporal: 2025-05-03 to 2025-05-07
  Confidence: 0.99
  Sources:
    - calendar_event: "Trip to Rome" (2025-05-03 to 2025-05-07)
    - photo: IMG_2045.jpg (GPS: 41.8902, 12.4924, 2025-05-05)
    - email: "Rate your Roman History Tour" (2025-05-08)

# Edit a fact
$ agent kb edit f-7a3b9c --confidence 1.0 --note "Verified by me"

# Forget something
$ agent kb forget "devansh likes pineapple pizza"

# Browse graph
$ agent kb browse --entity "devansh" --depth 2
```

### Obsidian-Compatible Export
The knowledge base can be exported as a folder of Markdown files with YAML frontmatter, compatible with Obsidian:

```markdown
---
entity_id: devansh
type: Person
aliases: ["Dev"]
---

# devansh

## Relationships
- [[lives_in]] → [[London]]
- [[works_as]] → [[Software Engineer]]
- [[visited]] → [[Rome]] (2025-05-03 to 2025-05-07)

## Sources
- calendar_event: Trip to Rome
- photo: IMG_2045.jpg
- email: Rate your Roman History Tour
```

This lets you browse your own knowledge graph in Obsidian, edit it, and sync changes back.

## Confidence Visualization
When browsing, facts are color-coded by confidence:
- **Green (>0.9):** High confidence, multiple corroborating sources
- **Yellow (0.7–0.9):** Medium confidence, single source or inferred
- **Red (<0.7):** Low confidence, unverified or contradicted

## Temporal Awareness
Facts are not just true/false — they exist in time. "devansh lives_in London" is true from 2023-01 onward, but before that it was "devansh lives_in Mumbai". The graph preserves history.
