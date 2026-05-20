# Journey: Flight Booking Auto-Add

## Trigger
User receives an email: "Your flight to Tokyo has been confirmed."

## Flow

### 1. Email Connector Syncs
- Gmail connector fetches new email
- Extracts structured data: flight number, dates, times, destinations

### 2. Knowledge Graph Update
- Creates/updates entities: `Flight JL043`, `Tokyo`, `Narita Airport`
- Creates fact: `devansh has_booking Flight JL043`
- Creates temporal fact: `Flight JL043 departs_at 2025-05-25T11:00:00Z`

### 3. Proactive Agent Detects
- Event Monitor sees new flight booking fact
- Pattern Recognizer: "New flight booking → usually add to calendar"
- Checks user preference: `auto_add_flight_to_calendar = true` (learned from past behavior)

### 4. Proactive Suggestion
```
🔔 I noticed a new flight booking:
   Japan Airlines JL043 to Tokyo
   May 25, 11:00 AM departure

I can add this to your calendar with:
- Check-in reminder (24h before)
- Departure reminder (2h before)
- Transit time estimate (1h30 to airport)

[Add to calendar] [Remind me later] [Don't add]
```

### 5. User Response Scenarios

**A. User says "Add it"**
- Calendar connector creates event
- Knowledge Graph updated: `calendar_event_added = true`
- Proactive Agent notes success

**B. User says "Don't add emails from Japan Airlines to my calendar"**
- Preference extracted: `auto_add_flight_to_calendar = false` for `sender = "japanairlines.com"`
- Future emails from this sender ignored for calendar auto-add
- Knowledge Graph stores preference with high confidence

**C. User ignores**
- Proactive Agent logs as ambiguous
- May prompt again if flight is imminent and still not in calendar
