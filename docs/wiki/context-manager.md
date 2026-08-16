# Conversation Context Manager

## What Is Persisted?

Mimir remembers every conversation you have.  Each chat is stored as a **session** containing:

- A **system prompt** that defines Mimir's personality and rules for that session.
- Every **user message** you send.
- Every **assistant message** Mimir generates.
- **Token counts** (when available) so Mimir knows how large the conversation is.
- **Timestamps** for audit and debugging.

All data lives in a single SQLite file on your device (`~/.local/share/mimir/context.db` by default).  The context database itself never leaves your machine, but messages are transmitted to remote LLM endpoints depending on your provider and configuration.  Review your LLM endpoint settings and provider privacy policies before using remote models.

## How Window Sizing Works

To keep LLM requests fast and within token limits, Mimir trims old conversation history automatically.  Two knobs control this:

| Setting | Default | What it does |
|---------|---------|-------------|
| `max_turns` | 20 | Hard cap on the number of back-and-forth exchanges kept. |
| `max_tokens` | 4096 | Soft cap on the total token count of the conversation. |

When either limit is exceeded, Mimir **drops the oldest complete pairs** of (user, assistant) messages.  The system prompt is never removed.

### Example

If `max_turns = 20` and you send 25 exchanges, the first 5 exchanges are deleted.  The system prompt plus the most recent 20 exchanges remain.

If token usage is known and the total exceeds `max_tokens`, Mimir drops oldest pairs until the count is back under budget.

## Configuring Limits

You can change the defaults in three ways (highest priority last):

1. **Edit `config/default.toml`** (affects new installations).
2. **Edit `~/.config/mimir/config.toml`** (user-wide override).
3. **Set environment variables** (temporary override):
   ```bash
   export MIMIR_CONTEXT_MAX_TOKENS=8192
   export MIMIR_CONTEXT_MAX_TURNS=50
   export MIMIR_CONTEXT_DB_PATH="/mnt/bigdisk/mimir/context.db"
   ```

## Persistence Behaviour

- **Sessions survive restarts.**  If you close and reopen Mimir, your conversation picks up where it left off.
- **Sessions are deleted** only when you explicitly call `delete_session` or erase the database file.
- **WAL mode** is enabled for safe concurrent reads and writes.

## Ephemeral vs. Stored Data

| Data | Stored? | Notes |
|------|---------|-------|
| System prompt | Yes | One per session. |
| User messages | Yes | With timestamp and optional token count. |
| Assistant messages | Yes | With timestamp and optional token count. |
| Raw LLM SSE chunks | No | Only the final text and usage are kept. |
| Intermediate reasoning | No | If verbose reasoning is enabled, only the final reply is stored. |

## Best Practices

- Keep `max_turns` modest (10–30) for most use-cases; LLM quality degrades with very long contexts anyway.
- If you run a local model with a small context window, lower `max_tokens` accordingly.
- Move `db_path` to a fast SSD if you notice latency during heavy usage.
