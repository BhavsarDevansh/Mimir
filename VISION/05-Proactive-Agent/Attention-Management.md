# Attention Management

## Philosophy
The user has limited attention. The agent competes with notifications, messages, and life. Every proactive interaction must earn its place. The agent should never flood the user with multiple suggestions at once.

## Attention Budget

The agent maintains an implicit "attention budget" for the user:
- **Urgent (red):** Safety, time-sensitive conflicts, imminent deadlines
  - Always interrupts immediately
  - Can bypass quiet hours for true emergencies
  
- **Important (orange):** Calendar conflicts, flight prep, forgotten items
  - Interrupts within 15 minutes if user is active
  - Batches if multiple within a short window
  
- **Helpful (blue):** Contextual reminders, price drops, suggestions
  - Delivered at natural breakpoints
  - Maximum 2 per hour
  
- **Optional (gray):** Interesting facts, non-urgent suggestions
  - Delivered in digest format
  - User reviews when convenient

## Batch and Digest Mode

When multiple proactive suggestions queue up, the agent batches them:

### Immediate Batch (if user is active)
```
🔔 A few things:

1. Flight to Tokyo in 6 hours. Want a checklist?
2. Calendar conflict at 2 PM: dentist vs. standup.
3. You are near a grocery store — you needed milk.

[Handle all] [Review individually] [Dismiss all]
```

### Digest Mode (if user is away)
```
🔔 While you were away:

- 9:00: Flight reminder (dismissed automatically, you checked in)
- 11:30: Price drop on Xbox (£349, 6-month low)
- 14:00: Calendar conflict resolved (standup moved to 3 PM)

Nothing needs your attention right now.
```

## Suppression Rules

The agent suppresses proactive suggestions when:
- User is in a meeting (calendar status = busy)
- User is driving (location + speed detected)
- User is in quiet hours (configurable)
- User recently said "do not disturb"
- Another high-priority notification was just sent (avoid flood)

## Priority Resolution

When multiple proactive events compete for attention:

```
Priority Order:
1. Safety emergencies (smoke, door unlocked, medical alert)
2. Time-critical conflicts (calendar overlap in < 1 hour)
3. Time-sensitive reminders (flight in < 4 hours)
4. Contextual opportunities (near store with needed item)
5. Pattern-based suggestions (packing reminder)
6. Optional information (price drops, recommendations)
```

If two events have equal priority, the more recent one wins.

## Smart Delivery

The agent learns when the user is most receptive:

```
Learned patterns:
- User responds to proactive messages within 5 minutes: 8–10 AM, 1–2 PM, 6–7 PM
- User ignores messages: 9 AM–12 PM (focus time), 10 PM+ (wind down)
- User prefers digests over individual notifications on weekdays
- User prefers immediate notifications on weekends
```

## The "Do Not Disturb" Contract

Users can set granular DND:

```bash
$ agent dnd until 5pm
Okay. I will hold all non-urgent notifications until 5 PM.
I will still alert you for emergencies.

$ agent dnd except calendar
Okay. I will hold everything except urgent calendar conflicts.
```

After DND ends, the agent delivers a digest:
```
🔔 DND ended. Here is what I held back:
- 3 price drop alerts (none expired)
- 1 suggested article on Roman history
- 1 reminder: your mum's birthday is tomorrow

[Review] [Dismiss all]
```

## Notification Fatigue Prevention

If the user dismisses 3+ proactive suggestions in a row without engagement:
1. Agent pauses proactive suggestions for 2 hours
2. Logs: "Possible notification fatigue detected"
3. Offers: "I notice you have been dismissing my suggestions. Want me to quiet down for a while?"
