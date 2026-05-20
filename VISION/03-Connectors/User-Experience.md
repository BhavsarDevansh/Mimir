# Connectors — User Experience

## What Is a Connector?
A Connector is a bridge between the agent and an external service (email, calendar, photos, etc.). Connectors read data from services, normalize it into facts, and optionally write back (e.g., add a calendar event).

## Onboarding Flow

### Adding a Connector
```bash
$ agent connector add gmail
Connector: Gmail
Required permissions: read emails, read labels
1. Open this URL in your browser:
   https://accounts.google.com/o/oauth2/auth?...
2. Paste the authorization code: ████████
3. Connected! Syncing emails from the last 30 days...

$ agent connector add homeassistant
Connector: Home Assistant
URL: http://homeassistant.local:8123
Long-lived access token: ████████
Connected! Found 47 entities (lights, sensors, cameras).
```

### Checking Status
```bash
$ agent connector status
Gmail              ● Online    Last sync: 2m ago    Emails: 12,304
Google Calendar    ● Online    Last sync: 5m ago    Events: 843
Apple Photos       ● Online    Last sync: 1h ago    Photos: 24,501
Home Assistant     ● Online    Last sync: 30s ago   Entities: 47
GitHub             ● Offline   Last sync: 1d ago    (token expired)
Signal             ○ Setup     (not configured)
```

### Managing Permissions
Each connector declares its permission levels:
- **Read-only:** Observe and learn
- **Read-write:** Observe and act (e.g., add calendar events)
- **Full:** Administrative actions (rare)

Users can revoke or downgrade permissions at any time:
```bash
$ agent connector permissions gmail --downgrade read-only
```

### Sync Control
```bash
$ agent connector sync gmail --full          # Full historical sync
$ agent connector sync gmail --since 7d      # Last 7 days only
$ agent connector pause github               # Pause syncing
$ agent connector resume github              # Resume syncing
```

## Data Privacy
- All data is fetched and stored locally
- No cloud intermediary (unless the service itself is cloud-based)
- Users can configure retention policies per connector
- "Forget everything from Gmail" is a supported operation
