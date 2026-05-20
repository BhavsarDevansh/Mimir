# Journey: Packing Reminder

## Trigger
Proactive Agent detects upcoming long-haul flight.

## Flow

### 1. Event Monitor Detects
- Calendar has flight: Tokyo, departing in 6 hours
- Pattern Recognizer: "Before long-haul flights, user usually packs 4 hours before"
- Current time: 6 hours before departure
- No packing activity detected (no home assistant signals of packing)

### 2. Context Gathering
- **Weather at destination:** Tokyo, 18°C, rain forecast
- **User's seasonal clothing:** Currently summer wardrobe
- **Home Assistant:** Storage box B3 contains winter clothes (from inventory)
- **Historical pattern:** User forgot umbrella on last Tokyo trip (from email/photos)

### 3. Proactive Suggestion Generation
```
🔔 Proactive: Flight Preparation

You have a flight to Tokyo (JL043) in 6 hours.

Context:
- 12-hour long-haul flight
- Tokyo: 18°C, rain expected
- You usually pack 4 hours before departure
- No packing detected yet

Suggestions:
- Pack noise-canceling headphones (you always use them)
- Bring an umbrella (rain forecast)
- Warmer jacket is in storage box B3
- Charge your AirPods (last used 2 days ago)

[Remind me in 1h] [I've packed] [Show full checklist]
```

### 4. User Response Scenarios

**A. "I've packed"**
- Log successful proactivity
- Increase confidence of packing pattern
- No further action

**B. "Remind me in 1h"**
- Schedule reminder
- If still no packing signal, escalate urgency

**C. "Show full checklist"**
- Generate personalized checklist based on:
  - Trip duration, weather, activities
  - Historical packing lists
  - Items last seen by Vision Tracking

**D. Ignore**
- Log as dismissed
- If flight time approaches and still no packing, send urgent reminder
