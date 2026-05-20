# Forgetting

## Philosophy
The user owns their data. They can forget anything, at any granularity, in natural language. Forgetting is reversible via a trash bin. The agent must understand and execute forget requests precisely.

## Granularity Levels

### 1. Single Fact
```bash
> "Forget that I said I like pineapple pizza."

Noted. I have removed: "devansh likes pineapple pizza"
Moved to trash. You can undo for 30 days.
```

```bash
$ agent kb forget --fact "devansh likes pineapple pizza"
Fact moved to trash. Undo with `agent kb undo f-7a3b9c`.
```

### 2. By Predicate
```bash
> "Forget everything you know about my medical history."

I have removed 23 facts related to your health:
  - allergies: peanuts, shellfish
  - medications: ...
  - conditions: ...

These are sensitive facts. Confirm: [Delete permanently] [Keep in trash]
```

```bash
$ agent kb forget --predicate "allergy" --subject "devansh"
12 facts moved to trash.
```

### 3. By Category
```bash
> "Forget everything about my relationships."

I have removed 34 relationship facts:
  - partner: Alice
  - sister: Priya
  - ...

[Undo] [Confirm permanent deletion]
```

```bash
$ agent kb forget --category relationships
34 facts moved to trash.
```

### 4. By Time Period
```bash
> "Forget everything I told you in January 2025."

I have removed 47 facts learned between 2025-01-01 and 2025-01-31.
Breakdown:
  - Explicit statements: 12
  - Extracted from emails: 23
  - Inferred: 12

[Undo] [Confirm]
```

```bash
$ agent kb forget --from 2025-01-01 --to 2025-01-31
47 facts moved to trash.
```

### 5. By Source
```bash
> "Forget everything you learned from my Gmail."

I have removed 312 facts originally extracted from Gmail.
This includes:
  - Travel: 45 facts
  - Events: 89 facts
  - Contacts: 34 facts
  - Purchases: 67 facts
  - Other: 77 facts

[Undo] [Confirm]
```

```bash
$ agent kb forget --source gmail
312 facts moved to trash.
```

### 6. By Entity
```bash
> "Forget everything about Alice."

I have removed 28 facts where Alice is the subject or object.
Note: This includes facts about your relationship with Alice.

[Undo] [Confirm]
```

```bash
$ agent kb forget --entity "Alice"
28 facts moved to trash.
```

### 7. Full Reset
```bash
> "Forget everything. Start over."

⚠️ This will remove ALL facts from your Knowledge Graph.
This includes 12,304 entities and 48,291 facts.
Your configuration and connectors will remain.

This action cannot be undone from the trash bin.

Type "DELETE EVERYTHING" to confirm: ████████

Knowledge Graph wiped. 48,291 facts deleted permanently.
The agent is now in observation-only mode.

To restore: `agent kb restore --from-backup`
```

```bash
$ agent kb reset --confirm
WARNING: This permanently deletes all knowledge.
Type "DELETE EVERYTHING" to confirm.
```

## Trash Bin

### How It Works
- Forgotten facts move to a trash bin (separate SQLite table)
- Trash retention: 30 days (configurable)
- Facts in trash are excluded from queries but recoverable
- After 30 days, permanently deleted

### Viewing Trash
```bash
$ agent kb trash
Trash bin (23 facts, expires in 28 days):

2025-05-20: "devansh likes pineapple pizza" (user command)
2025-05-19: "devansh visited Rome 2022-04-12" (correction)
2025-05-18: "Alice phone +44 7700 900123" (user command)
...

[Restore all] [Restore selected] [Empty trash now]
```

### Restoring
```bash
$ agent kb restore "devansh likes pineapple pizza"
Restored: "devansh likes pineapple pizza"
```

```bash
$ agent kb restore --all
Restored 23 facts from trash.
```

### Auto-Cleanup
```bash
$ agent kb trash --auto-cleanup 7d
Trash will be emptied after 7 days instead of 30.
```

## Undo and Redo

### Immediate Undo
```bash
> "Oops, do not forget that."

I have restored the fact: "devansh likes pineapple pizza"
```

```bash
$ agent kb undo
Undoing last forget operation...
Restored 1 fact.
```

### Multi-Step Undo
```bash
$ agent kb undo --steps 3
Undoing last 3 forget operations...
Restored 5 facts, 1 connector removal.
```

### Redo
```bash
$ agent kb redo
Redoing last undone operation...
Removed 1 fact (restored to trash).
```

## Natural Language Forget Requests

The agent parses natural language forget requests precisely:

| User Says | Agent Action |
|-----------|-------------|
| "Forget I said that" | Removes most recently added fact from current conversation |
| "Forget everything about X" | Removes all facts where X is subject or object |
| "Forget what you learned from my emails" | Removes all facts with source_type = email |
| "Forget everything from last month" | Removes facts learned in last 30 days |
| "Delete my medical data" | Removes health category + sensitive flag check |
| "Erase Alice from my knowledge" | Removes entity "Alice" and all related facts |
| "Start fresh" | Warns, then wipes Knowledge Graph |

## Cascading Effects of Forgetting

When facts are forgotten, the agent handles downstream effects:

### Inferred Facts
- If an inferred fact depended ONLY on the forgotten fact → also delete inferred fact
- If an inferred fact had multiple sources → re-evaluate, reduce confidence

### Permissions
- If a permission was learned from a forgotten pattern → flag for review
- If permission was explicitly granted by user → retain

### Patterns
- Patterns derived from forgotten facts → reduce confidence or delete if no longer supported

### Example
```
User: "Forget everything about my trip to Rome 2022."

Agent:
  - Removed: "devansh visited Rome 2022-04-12"
  - Removed: "devansh visited Colosseum 2022-04-12" (inferred, only source was Rome visit)
  - Updated: "devansh visited Colosseum" (confidence reduced, now based on 2025 visit only)
  - Retained: "devansh visited Rome 2025-05" (independent fact)
```

## Security Considerations

- Full reset requires explicit confirmation phrase
- Bulk deletions (>100 facts) require confirmation
- Sensitive category deletions (health, financial) require extra confirmation
- Audit log records all forget operations with reason
- No external service notified when facts are forgotten (local-only operation)
