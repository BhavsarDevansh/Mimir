# Proactive Agent — User Experience

## Philosophy: Earned Trust Over Time

The agent does not start by being proactive. It starts by observing. Only after it understands patterns and has evidence that a proactive suggestion would be welcome does it offer to help. The user climbs the trust ladder at their own pace.

## The Trust Ladder

### Stage 1: Observation (Days 1–7)
- Agent connects to services and silently observes
- Facts are stored, patterns are learned
- No proactive suggestions are made
- User may query the agent, but the agent does not initiate

### Stage 2: Gentle Offers (Days 7–30)
- Agent detects its first opportunities to help
- Offers are infrequent, low-stakes, and explicitly framed as offers
- User can accept, reject, or defer
- Every response teaches the agent

```
🔔 First Offer

I noticed you have an email for a flight to Tokyo but it is not in your calendar.
You have done this manually 3 times in the past month.

Would you like me to add it for you?
[Add it] [Not now] [Never suggest this]
```

### Stage 3: Pattern-Based Permissions (Days 30–90)
- Agent identifies recurring patterns with high confidence
- Offers category-level permissions
- User can grant once, always, or with conditions

```
🔔 Permission Offer

I have found 8 flight emails in the last month and you added 7 to your calendar.
Would you like me to automatically add flight events from now on?

[Yes, always] [Yes, but ask first for non-flights] [No, keep asking each time]
```

### Stage 4: Autonomous Assistance (90+ Days)
- Agent acts within granted permissions
- Still asks for anything outside known patterns
- User can escalate or de-escalate autonomy at any time
- Agent periodically checks in: "Are my calendar additions still helpful?"

## Proactivity Levels

### Never
The agent only responds when explicitly asked. No notifications, no suggestions.

### Important Only (Default)
The agent interrupts only for high-confidence, time-sensitive, high-impact events:
- Upcoming flights with preparation needed
- Conflicts in calendar
- Forgetting something you explicitly said was important
- Safety-critical home events (smoke alarm, door left unlocked)

### Contextual
The agent surfaces helpful context when it detects you might need it:
- "You are near the grocery store, you needed milk"
- "Your meeting with Alice is in 10 minutes, here is the last email thread"

### Always
The agent is maximally helpful, even for low-confidence suggestions:
- "You have not listened to your Discover Weekly yet"
- "It has been 3 days since you messaged your mom"

## Notification Patterns

### Example: First Offer (Flight)
```
🔔 I noticed something:

You have a flight to Tokyo (JL043) on May 25.
I found the confirmation email but it is not in your calendar yet.

You have manually added 3 of 4 recent flight emails.

[Add to calendar] [Remind me later] [I will handle it]
```

### Example: Permission Offer (Calendar)
```
🔔 Permission Offer

I have helped you add 6 events from emails to your calendar in the last month.
You accepted 5 and rejected 1.

Would you like me to automatically add events from emails going forward?
I will still notify you after I do, so you can review or undo.

[Yes, auto-add] [Keep asking each time] [Not now]
```

### Example: Proactive Reminder (Established Trust)
```
🔔 Flight Preparation

You have a flight to Tokyo (JL043) departing in 6 hours.

Based on your history:
- This is a 12-hour long-haul flight
- Tokyo forecast: 18°C and rain
- You usually pack noise-canceling headphones
- Your warmer clothes are in storage box B3

[Show checklist] [Remind me in 1h] [I have packed]
```

### Example: Calendar Conflict
```
⚠️ Calendar Conflict Detected

You have two events at 2 PM:
- "Dentist appointment" (confirmed)
- "Team standup" (recurring)

I can:
- Move the standup to 3 PM (check availability)
- Suggest you decline the standup
- Remind you 30 minutes before to decide

[Move standup] [Decline standup] [Remind me later]
```

## Learning from User Response

Every proactive interaction is a learning signal:

| User Action | Learning |
|-------------|----------|
| Ignored without action | Reduce confidence of this trigger |
| "Not now" | Temporarily suppress this type |
| "Do not ask again" | Create negative preference |
| Accepted and acted on | Increase confidence, may offer blanket permission |
| Edited suggestion | Learn the correction |
| "Always do this" | Grant category permission |

## Proactive Audit Trail
```bash
$ agent proactive history
2025-05-20 08:00: Flight reminder (accepted)
2025-05-19 14:30: Suggested rescheduling (ignored)
2025-05-18 09:00: Weather-appropriate clothing (accepted)
2025-05-17 11:00: Permission offer — calendar auto-add (accepted)
```

## User Control
```bash
# Temporarily suppress all proactivity
$ agent proactive pause 2h

# Permanently disable a category
$ agent proactive disable weather_suggestions

# View what would trigger
$ agent proactive preview

# Reset to observation-only
$ agent proactive reset
```

## Sensitivity-Aware Proactivity

The agent modulates its proactivity based on context:

### Private Context (User Alone)
- Full proactivity enabled
- All topics permitted
- Detailed suggestions

### Public or Shared Context (Others Present)
- Suppresses sensitive topics (medical, financial, relationship)
- Reduces verbosity
- Avoids mentioning specific private facts

### Explicit Sensitivity Boundaries
- User says: "Do not learn about my medical details"
- Agent stops extracting medical facts
- Deletes existing medical facts from KB
- Creates negative permission
- Never mentions medical topics unless explicitly asked
