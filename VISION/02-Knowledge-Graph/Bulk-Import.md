# Bulk Import

## Philosophy
Users should be able to seed the agent's knowledge with existing information. The agent must handle diverse formats, detect conflicts with existing facts, and treat imported facts appropriately based on source and content.

## Supported Formats

### 1. Plain Text
Simple key-value or sentence-style facts.

```text
# facts.txt
My name is Devansh.
I live in London.
I work as a software engineer.
My favourite colour is blue.
I am allergic to peanuts.
My sister's birthday is March 15th.
I prefer aisle seats on flights.
```

**Import:**
```bash
$ agent kb import facts.txt --format plaintext
Importing facts from facts.txt...
- Extracted 7 facts
- 2 are sensitive (allergy, birthday) — requiring confirmation
- 0 conflicts with existing facts
- Preview? [Yes] [Import directly]
```

### 2. Obsidian Vault
Markdown files with YAML frontmatter.

```markdown
---
entity_id: devansh
type: Person
---

# devansh

## Facts
- favourite_colour: blue
- lives_in: London
- works_as: Software Engineer
- allergy: peanuts
- sister_birthday: March 15th
- flight_preference: aisle seat
```

**Import:**
```bash
$ agent kb import ~/Obsidian/Personal --format obsidian
Scanning 47 markdown files...
- Found 12 entities
- Extracted 34 facts
- 3 sensitive facts flagged for confirmation
- Conflicts detected: 1 (existing: "devansh lives_in Manchester")
[Resolve conflicts] [Import non-conflicting only] [Cancel]
```

### 3. Spreadsheet (CSV/TSV)
Structured tabular data.

```csv
subject,predicate,object,confidence,source
devansh,favourite_colour,blue,1.0,user_import
devansh,lives_in,London,1.0,user_import
devansh,allergy,peanuts,1.0,user_import
devansh,flight_preference,aisle,0.9,user_import
```

**Import:**
```bash
$ agent kb import facts.csv --format csv --delimiter ,
Importing 4 facts...
- All facts have explicit confidence >= 0.90
- 1 sensitive fact flagged (allergy)
- Proceed? [Yes] [Preview] [Cancel]
```

### 4. JSON
Machine-readable structured import.

```json
{
  "entities": [
    { "id": "devansh", "name": "Devansh", "type": "Person", "aliases": ["Dev"] }
  ],
  "facts": [
    { "subject": "devansh", "predicate": "favourite_colour", "object": "blue", "confidence": 1.0 },
    { "subject": "devansh", "predicate": "allergy", "object": "peanuts", "confidence": 1.0, "sensitive": true }
  ]
}
```

## Import Workflow

### Step 1: Parse and Extract
- Parse file format
- Extract structured facts
- Validate schema (subject, predicate, object present)

### Step 2: Conflict Detection
- Check for existing facts with same subject + predicate
- Flag conflicts where new fact differs from existing
- Classify sensitivity (health, financial, etc.)

### Step 3: Preview
Present a preview to the user:
```
Preview of import (7 facts):

✅ New facts (5):
  - devansh works_as Software Engineer
  - devansh favourite_colour blue
  - devansh flight_preference aisle seat
  - devansh sister_birthday March 15th
  - devansh lives_in London

⚠️ Sensitive facts requiring confirmation (1):
  - devansh allergy peanuts

❌ Conflicts with existing facts (1):
  - devansh lives_in: NEW = London, EXISTING = Manchester
    [Keep existing] [Use new] [Keep both]

[Import all] [Import confirmed only] [Cancel]
```

### Step 4: Import
- Insert new facts with `source_type: user_import`
- Overwrite existing facts per Learning-Modes rules
- Flag sensitive facts for confirmation
- Log all changes in audit trail

### Step 5: Confirmation (Sensitive Facts)
If sensitive facts are imported, the agent requires explicit confirmation before treating them as valid:
```
⚠️ I imported a sensitive fact: "You have a PEANUT ALLERGY."
Please confirm this is correct before I use it in suggestions.

[Confirm] [Edit] [Delete]
```

Sensitive facts remain in `pending_confirmation` state until confirmed.

## Obsidian Bidirectional Sync

### Import from Obsidian
```bash
$ agent kb import ~/Obsidian/Vault --format obsidian --watch
Watching ~/Obsidian/Vault for changes...
New fact detected in note "Travel Preferences.md":
  - devansh flight_preference: window seat (was: aisle seat)

Overwrite? [Yes] [Keep both] [Ignore]
```

### Export to Obsidian
```bash
$ agent kb export ~/Obsidian/Vault/Agent-Knowledge --format obsidian
Exported 1,247 entities and 4,832 facts to ~/Obsidian/Vault/Agent-Knowledge/
```

Changes in Obsidian are detected and re-imported:
- User edits a fact in Obsidian → Agent detects change on next sync
- Treated as `source_type: user_edit` with confidence 1.0
- Overwrites existing facts per Learning-Modes rules

## Handling Conflicts

When imported facts conflict with existing facts:

| Scenario | Action |
|----------|--------|
| New explicit vs. old inferred | Overwrite old |
| New explicit vs. old explicit | Ask user |
| New connector vs. old explicit | Ask user |
| New casual vs. old anything | Coexist, new marked as casual |
| Same value, different confidence | Keep higher confidence |

## Technology
- **Plaintext parsing:** Custom NLP extraction (LLM-assisted for unstructured text)
- **Obsidian parsing:** YAML frontmatter + wiki-link extraction
- **CSV/TSV:** Standard CSV parser with header mapping
- **JSON:** serde deserialization
- **File watching:** notify crate or inotify/kqueue
