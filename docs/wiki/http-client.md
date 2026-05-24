# HTTP Client and Stateless Chat

## What Changed in v0.13.0

The `mimir` binary no longer loads `mimir-core` for chat commands. Instead, `mimir ask`, `mimir chat`, `mimir status`, and `mimir memory` talk to the daemon via HTTP.

## Why?

- **Faster startup** — No need to initialise the LLM client, context database, or memory loader in the CLI process.
- **Shared state** — The daemon owns all sessions, so multiple clients (CLI, web UI, future connectors) see the same conversation history.
- **Simpler architecture** — The CLI is a thin client; the daemon is the sole source of truth.

## How It Works

```text
mimir ask "hello"     →  HTTP POST /chat      →  daemon
mimir chat            →  HTTP POST /chat      →  daemon (REPL loop)
mimir status          →  HTTP GET /status     →  daemon
mimir memory          →  HTTP GET /memory     →  daemon
```

All commands default to `http://127.0.0.1:8080`. If the daemon is not running, the command will fail with a connection error.

## Stateless REPL

`mimir chat` no longer keeps a local `conversation` vector. The daemon stores every message. The REPL only remembers the `session_id` returned by the server:

1. Start `mimir chat` — no session ID is sent.
2. The server creates a new session and returns its ID.
3. The client saves this ID.
4. Every subsequent message includes the same `session_id`.
5. Type `/clear` — the client drops the ID, so the next turn creates a new session.

## Resuming Conversations

Because the session lives on the server, you can resume a conversation by reusing the session ID. Currently this requires passing the ID manually (e.g., via a future flag or by storing it yourself). The REPL does not persist the session ID across restarts.

## Per-Request Overrides

When using `mimir ask`, the following flags are sent to the server as part of the `ChatRequest`:

- `--model` → `model` field — overrides the configured LLM model for that request only.
- `--personality` → `personality_preset` field — uses a different personality preset for that request only.
- `--incognito` → `incognito` field — skips all database persistence. No session is created, and no messages are stored.

These overrides do not affect the daemon's global configuration.
