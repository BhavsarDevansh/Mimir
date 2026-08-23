# Supported Services

## Tier 1: Core (Phase 1-2)
These are essential for basic functionality and should be built first.

### Email (IMAP)

- **Read:** Emails, headers, attachments metadata
- **Extract:** Flight confirmations, bookings, receipts, addresses, dates, contacts
- **Write:** None (read-only for safety)
- **Auth:** OAuth 2.0 or IMAP password
- **Complexity:** Medium

### Google Calendar / Apple Calendar / CalDAV
- **Read:** Events, locations, attendees, recurrences
- **Extract:** Travel plans, meetings, birthdays, recurring activities
- **Write:** Add events, update events, delete events
- **Auth:** OAuth 2.0 or app-specific passwords
- **Complexity:** Medium

### Apple Photos / Google Photos / Local Photos
- **Read:** Photo metadata, EXIF, GPS, timestamps
- **Extract:** Locations visited, objects detected, people, events
- **Write:** None
- **Auth:** Local file system or OAuth 2.0
- **Complexity:** Medium (face recognition is hard)

## Tier 2: Extended (Phase 3-4)

### GitHub
- **Read:** Repos, commits, PRs, issues, stars
- **Extract:** Projects worked on, technologies used, coding patterns
- **Write:** Create issues, comments (optional)
- **Auth:** Personal Access Token
- **Complexity:** Low

### Spotify / Apple Music
- **Read:** Playlists, listening history, liked songs
- **Extract:** Music taste, moods, activities associated with music
- **Write:** Create playlists (optional)
- **Auth:** OAuth 2.0
- **Complexity:** Low

### Home Assistant
- **Read:** Sensor states, events, automations
- **Extract:** Home occupancy, temperature preferences, routines
- **Write:** Trigger automations, set states
- **Auth:** Long-lived access token
- **Complexity:** Medium

### Signal
- **Read:** Message history (as linked device)
- **Extract:** Conversations, plans, shared locations
- **Write:** Send messages (optional)
- **Auth:** QR code link
- **Complexity:** High (E2EE complexity)

## Tier 3: Advanced (Phase 5+)

### Banking / Financial (Plaid, Open Banking)
- **Read:** Transactions, balances
- **Extract:** Spending patterns, subscriptions, financial health
- **Write:** None
- **Auth:** OAuth 2.0 / Open Banking
- **Complexity:** High (security, compliance)

### Health (Apple Health, Fitbit)
- **Read:** Steps, sleep, heart rate, workouts
- **Extract:** Activity levels, sleep patterns, health trends
- **Write:** None
- **Auth:** OAuth 2.0 / HealthKit
- **Complexity:** Medium

### Social Media (Twitter/X, LinkedIn, Instagram)
- **Read:** Posts, connections, interactions
- **Extract:** Network, interests, public activities
- **Write:** None (read-only by design)
- **Auth:** OAuth 2.0
- **Complexity:** Low (API availability varies)

### Browser History (Local)
- **Read:** Visited URLs, timestamps
- **Extract:** Research topics, interests, reading habits
- **Write:** None
- **Auth:** Local file access
- **Complexity:** Low

## Future / Wishlist
- **Amazon / E-commerce:** Purchase history, delivery tracking
- **Netflix / Streaming:** Watch history, preferences
- **Uber / Maps:** Trip history, frequent destinations
- **Notion / Obsidian:** Notes, knowledge
- **Slack / Discord:** Work/personal communications
