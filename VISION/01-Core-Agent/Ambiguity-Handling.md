# Ambiguity Handling

## Philosophy
When the agent is uncertain, it asks. When the cost of inaction is high (e.g., missing a flight), it acts with a notification. The user is never left wondering why something happened or didn't happen.

## Decision Rules

### Ask When in Doubt
If confidence is below the action threshold, the agent asks the user rather than guessing.

Examples:
- Email might be a flight confirmation but sender name does not match → Ask
- Two meetings today and user says "remind me about the meeting" → Ask which one
- Calendar event detected but it looks like a friend event, not user's → Ask

### Act When Cost of Inaction is High
If not acting could cause real problems, the agent acts and notifies.

Examples:
- Flight in 6 hours and no packing signal detected → Remind to pack
- Calendar conflict in 1 hour → Alert immediately
- Safety-critical home event (smoke alarm) → Alert immediately

### Natural Language Examples

```
User: "Remind me about the meeting"
Agent: "You have two meetings today:
  1. Team standup at 10 AM
  2. Project review at 2 PM
  Which one?"
```

```
Agent: "I found an email that might be a flight confirmation,
but the passenger name does not match yours.
Should I add it to your calendar?"
```

```
Agent: "I noticed you have a flight to Tokyo in 6 hours.
I have not detected any packing activity.
I am reminding you to pack — want a checklist?"
```

## Multi-Channel Communication

The agent should not rely on a chat interface being open. If the user has Signal configured, it sends a Signal message. If email is configured, it can send an email. The user configures preferred channels.

```toml
[notifications]
channels = ["signal", "email"]
priority_high = "signal"
priority_low = "email"
```

The agent uses the highest-priority available channel for urgent notifications.
