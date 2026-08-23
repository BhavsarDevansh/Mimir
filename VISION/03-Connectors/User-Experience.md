# Connectors — User Experience

## What Is a Connector?
A Connector is a bridge between the agent and an external service (email, calendar, photos, etc.). Connectors read data from services, normalize it into facts, and optionally write back (e.g., add a calendar event).

## Onboarding Flow

### Adding a Connector

The default path is the interactive wizard — `mimir connector add` with no arguments lists the daemon's supported `(connector_type, backend)` pairs for selection, confirms the display name (defaults to the type) and slug (defaults to the slugified name), asks the per-backend questions with sensible defaults, and drives authentication. Email presets (issue #400) pre-fill the provider defaults — Gmail, Outlook / Office 365, Yahoo, Proton Mail (Bridge), iCloud, or Custom IMAP — and calendar presets cover Google Calendar, iCloud, Yahoo, and Custom CalDAV. For Gmail IMAP the wizard offers OAuth browser login first (Google authorization/token endpoints pre-filled; the user supplies their own OAuth client ID from the Google Cloud Console), launching the browser at the printed authorize URL — the URL can also be opened manually, but the browser must run on the machine running `mimir` because the PKCE callback binds to `127.0.0.1` — with an app-password fallback; Outlook pre-fills the Microsoft identity platform endpoints and IMAP scope (app password first), and Yahoo / Proton / iCloud are app-password-only. Local backends (Photos) need no credential.

```bash
$ mimir connector add
Connector type: Email (imap)
Email provider: [Gmail | Outlook / Office 365 | Yahoo | Proton Mail (Bridge) | iCloud | Custom IMAP]
Display name (Email):
Slug (email):
IMAP server host (imap.gmail.com):
IMAP port (blank = 993):
Mailbox (INBOX):
Account email (IMAP login):
Sync mode: [Continuously — push (recommended) | Every N minutes — polling]
First sync: [Import existing mailbox content (recommended) | Only new content from now on]
...
Authentication: [OAuth 2.0 — browser login (recommended)]
OAuth client ID (Google Cloud Console → Credentials → OAuth client): ...
Required permissions: read emails, read labels
If the browser does not open automatically, visit:
  https://accounts.google.com/o/oauth2/v2/auth?client_id=...&code_challenge=...&state=...
[Browser opens; the user authorizes; the provider redirects to the loopback callback]
Added connector 'email' (email / imap, status setup, auth authenticated).
Next: run `mimir connector resume email` to activate it, then `mimir connector sync email` to sync.

$ mimir connector add photos --backend local watch_dir=/home/me/Pictures
Added connector 'photos' (photos / local, status setup, auth unauthenticated).
Next: run `mimir connector resume photos` to activate it, then `mimir connector sync photos` to sync.
```

The flag form (`mimir connector add email --backend imap auth.kind=app_password auth.username=me@example.com --password-stdin …`; the legacy `gmail` type name still registers the same connector) remains for scripts; non-OAuth flows are fully non-interactive — whether credentials are supplied or the backend is credential-free local like the Photos command above — while `auth.kind=oauth` still opens the browser for PKCE and waits for the loopback callback. It runs the same registration and credential-ingest core. The OAuth flow (A4 / #205) runs entirely in the CLI process: it binds an ephemeral loopback listener on `127.0.0.1`, opens the provider's authorize URL in the default browser (the URL is printed first, so it can also be opened manually — but the browser must run on the machine running `mimir`, because the callback binds to `127.0.0.1`), receives the redirect, exchanges the code, and POSTs the token bundle to the daemon — the user never copies a code. A canceled flow exits with nothing created.

### Checking Status
```bash
$ mimir connector status
Email              ● Online    Last sync: 2m ago    Emails: 12,304
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
$ mimir connector permissions email --downgrade read-only
```

### Sync Control
```bash
$ mimir connector sync email --full          # Full historical sync
$ mimir connector sync email --since 7d      # Last 7 days only
$ mimir connector pause github               # Pause syncing
$ mimir connector resume github              # Resume syncing
```

## Data Privacy
- All data is fetched and stored locally
- No cloud intermediary (unless the service itself is cloud-based)
- Users can configure retention policies per connector
- "Forget everything from a connector" is a supported operation
