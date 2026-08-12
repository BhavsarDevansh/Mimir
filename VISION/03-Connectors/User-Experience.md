# Connectors — User Experience

## What Is a Connector?
A Connector is a bridge between the agent and an external service (email, calendar, photos, etc.). Connectors read data from services, normalize it into facts, and optionally write back (e.g., add a calendar event).

## Onboarding Flow

### Adding a Connector
```bash
$ mimir connector add gmail --backend <b> auth.kind=oauth auth.auth_uri=https://accounts.google.com/o/oauth2/v2/auth auth.token_endpoint=https://oauth2.googleapis.com/token auth.client_id=... auth.username=you@gmail.com
Connector: Gmail
Required permissions: read emails, read labels
If the browser does not open automatically, visit:
  https://accounts.google.com/o/oauth2/v2/auth?client_id=...&code_challenge=...&state=...
[Browser opens; the user authorizes; the provider redirects to the loopback callback]
Connected! Syncing emails from the last 30 days...

$ agent connector add homeassistant
Connector: Home Assistant
URL: http://homeassistant.local:8123
Long-lived access token: ████████
Connected! Found 47 entities (lights, sensors, cameras).
```
The OAuth flow (A4 / #205) runs entirely in the CLI process: it binds an ephemeral loopback listener on `127.0.0.1`, opens the provider's authorize URL in the default browser (the URL is printed first, so headless/SSH sessions can open it manually), receives the redirect, exchanges the code, and POSTs the token bundle to the daemon — the user never copies a code. A canceled flow exits with nothing created.

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
