# Multi-Device Experience

## Current Scope: Single User, Multiple Devices

The agent is designed for a single user who may access it from multiple devices (phone, laptop, desktop, tablet). The home server is the source of truth.

## Device Roles

### Primary Device: Home Server
- Runs the agent daemon 24/7
- Hosts the Knowledge Graph and all connectors
- Handles sync, reasoning, and proactive monitoring
- Exposes API to secondary devices

### Secondary Devices: Phone, Laptop, Desktop
- Run lightweight clients (CLI, TUI, or web UI)
- Connect to the home server via local network or secure tunnel
- Cache recent conversation context for offline viewing
- Cannot run connectors or reasoning independently

## Current Implementation (issue #388)

The daemon now exposes an OpenAI-compatible provider surface — `GET /v1/models` and `POST /v1/chat/completions` (blocking + streaming) — so any app or device that speaks the OpenAI chat-completions API can point at the central Mimir server. Mimir is single-tenant, so there is no device identity: the OpenAI `user` field is a conversation key, and a fixed `user` value resumes one ongoing conversation in the central profile, while requests without `user` (or with a blank one) key the fixed `default` session. Every request is persistent and every completed turn fires the learning hooks, exactly as this vision assumes — there is no incognito path on this surface (issue #473). See `docs/llm-provider.md` and `docs/wiki/llm-provider.md`.

## Connection Methods

### Local Network
```
Phone/Laptop/Desktop → WiFi/LAN → Home Server (agent-daemon:8080)
```
- Fast, no internet required
- Automatic discovery via mDNS/Bonjour (future)
- TLS with client certificates (future; today the daemon is reverse-proxy-first and authenticates with its bearer API token)

### Remote Access
```
Phone (4G/5G) → Internet → Home Router → Home Server
```
- Reverse proxy with Tailscale, WireGuard, or similar
- Or: Cloudflare Tunnel for zero-config remote access
- All data stays on home server; phone is just a viewport

## Sync Model

### Conversation Sync
- All conversations stored on home server
- Any device can resume any conversation (today: a fixed OpenAI `user` key resumes the conversation on the provider surface)
- Real-time sync via WebSocket (future)
- Offline: last N conversations cached locally

### Notification Sync
- Proactive notifications pushed to all connected devices
- User can configure which devices receive which notifications:
  ```toml
  [notifications.devices]
  phone = ["urgent", "calendar", "location"]
  laptop = ["all"]
  desktop = ["proactive", "calendar"]
  ```

### Quick Capture
From any device, user can quickly capture a thought or fact:
```bash
# On phone via shortcut
$ agent capture "Remember to buy milk"
# Stored immediately to Knowledge Graph
```

## Device-Specific Behavior

The agent can adapt based on which device the user is interacting from:

### Phone
- Shorter responses by default
- Voice input support
- Location-aware queries
- Quick-action notifications (tap to confirm)

### Laptop/Desktop
- Verbose mode available
- Full reasoning trail display
- Knowledge Graph browsing
- Connector management

### Tablet
- Middle ground between phone and desktop
- Good for reviewing investigations

## Context Awareness

The agent infers the user's current context from which device is active:

```
User asks from phone at 8 AM: "What is today like?"
→ Prioritize: calendar, commute, weather

User asks from laptop at 10 PM: "What is today like?"
→ Prioritize: tomorrow's schedule, outstanding tasks, wind-down
```

## Future: Shared Household Setup

If the agent evolves to support multiple users in a household:
- Each user has isolated Knowledge Graph partition
- Shared facts (e.g., "house temperature") are in shared space
- Proactive suggestions respect which user is currently active
- Cross-user permissions (e.g., "my partner can see my calendar events")

## Security

- Inter-device communication is encrypted when traffic passes through a TLS-terminating reverse proxy. Direct daemon binds use bearer-token authentication and do not provide TLS.
- No raw data leaves the home server
- Device tokens for authentication (future; today every device presents the daemon's single bearer API token, issue #281)
- Revocable per-device access (future)
