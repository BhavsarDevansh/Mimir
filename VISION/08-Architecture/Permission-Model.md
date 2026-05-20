# Permission and Consent Model

## Philosophy: The Trust Ladder

The agent does not ask for broad permissions upfront. Instead, it observes, learns, and offers specific, contextual permissions when it has evidence they would be useful. The user climbs the trust ladder at their own pace — or can jump straight to the top with blanket authority.

## Consent Levels

### Level 0: Observation Only
- Agent reads data from connectors
- Stores facts in Knowledge Graph
- Never acts on the user's behalf
- Proactive suggestions are purely informational
- Default state for new connectors

### Level 1: Ask Every Time
- Agent detects an opportunity to help
- Asks explicitly before acting
- User confirms or rejects each individual action
- Agent learns from responses

### Level 2: Category Permission
- User grants permission for a class of actions
- Example: "Yes, always add flight emails to my calendar"
- Agent acts automatically within that category
- Still asks for anything outside the category

### Level 3: Domain Permission
- User grants permission for a whole domain
- Example: "Automatically manage my calendar based on any relevant information"
- Agent acts across email, messages, and any other source for calendar actions
- Still asks for actions outside calendar management

### Level 4: Full Authority
- User grants blanket permission for all low-risk actions
- Agent acts proactively without asking
- Still asks for high-risk or irreversible actions (e.g., sending messages, financial transactions)
- User can revoke or downgrade at any time

## Granting Permissions

### Contextual Offers (The Default Path)
After observing a pattern, the agent makes a contextual offer:

```
🔔 Permission Offer

I noticed you have an email for a flight but it is not in your calendar.
In the last month, I found 3 flight emails and you manually added 2 to your calendar.

Would you like me to:
[Add this one only]          [Always add flight emails]  
[Ask me each time]            [Never offer this again]
```

### Upfront Declaration (Power User Path)
Users can declare authority immediately:

```bash
$ agent authorize calendar --scope all --from any_source
Granted: Agent may read any connected source and add/update/delete calendar events.
Scope: all_sources
Risk level: medium
Revoke: agent revoke calendar
```

Or via natural language:
> "Automatically add events to my calendar from any source."
> 
> Understood. I will add events to your calendar from emails, messages, and any other source without asking.
> I will still notify you after I do so you can review.
> Say "stop auto-adding calendar events" at any time to revoke.

## Permission Storage

Permissions are stored as high-confidence preferences in the Knowledge Graph:

```rust
struct Permission {
    id: String,
    category: String,        // e.g., "calendar_auto_add"
    scope: PermissionScope,  // specific | category | domain | full
    sources: Vec<String>,    // e.g., ["gmail", "any"]
    conditions: Vec<Condition>, // e.g., sender_filter, confidence_threshold
    granted_at: DateTime,
    revoked_at: Option<DateTime>,
    granted_explicitly: bool, // true = user said upfront; false = learned from pattern
    grant_context: String,   // The interaction that led to this permission
}
```

## Revocation

Permissions are revocable at any time:

```bash
$ agent revoke calendar_auto_add
Revoked: Auto-adding events from emails.

$ agent revoke all
Revoked: All permissions. Agent reverted to observation-only mode.
```

Natural language revocation works too:
> "Stop adding emails to my calendar."
> 
> Noted. I will no longer add events from emails to your calendar.
> I have deleted the permission. Events I already added remain.

## Sensitivity-Aware Permissions

Some permissions are sensitive and require additional safeguards:

### Medical Data
- Stored by default (local-first)
- Never mentioned in public contexts (ambient detection)
- User can say: "Do not learn about my medical details"
- Agent deletes all medical facts and creates a negative permission

### Financial Data
- Read-only by default
- Write actions (e.g., bill payment) always require explicit confirmation
- Never auto-act even with full authority

### Relationships and Private Communications
- Never summarized or quoted without permission
- Can be used for inference but not surfaced in casual conversation

## Audit of Permissions

```bash
$ agent permissions list
Permission                          Scope           Sources        Status
─────────────────────────────────────────────────────────────────────────
calendar_auto_add                   domain          any            active
calendar_auto_add.sender_filter     specific        !boss@company  active
flight_checkin_reminder             category        calendar       active
medical_data_ingestion              domain          any            revoked
```

```bash
$ agent permissions audit
2025-05-10: Granted calendar_auto_add (context: flight email detected)
2025-05-12: Revoked medical_data_ingestion (context: user said "don't learn my medical details")
2025-05-15: Modified calendar_auto_add (context: user said "except from my boss")
```

## Learning Permissions from Behavior

When a user consistently accepts or rejects suggestions, the agent may infer a permission:

```
🔔 Permission Inference

I have asked you 5 times if you want flight emails added to your calendar.
You said yes every time.

Would you like me to just do this automatically from now on?
[Yes, always] [No, keep asking] [Review past decisions]
```

The agent never creates a permission without explicit confirmation, even if the pattern is strong.
