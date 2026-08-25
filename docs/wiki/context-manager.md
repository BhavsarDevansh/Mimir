# Conversation Context Manager

## What Is Persisted?

Mimir remembers every conversation you have.  Each chat is stored as a **session** containing:

- A **system prompt** that defines Mimir's personality and rules for that session.
- Every **user message** you send.
- Every **assistant message** Mimir generates.
- Assistant **tool-call messages** and the **tool results** that follow them, so agentic turns round-trip correctly across requests.
- **Token counts** (when available) so Mimir knows how large the conversation is.
- **Timestamps** for audit and debugging.

All data lives in a single SQLite file on your device (`~/.local/share/mimir/context.db` by default).  The context database itself never leaves your machine, but messages are transmitted to remote LLM endpoints depending on your provider and configuration.  Review your LLM endpoint settings and provider privacy policies before using remote models.

## How Window Sizing Works

To keep LLM requests fast and within token limits, Mimir keeps a bounded window of conversation history and summarises what falls out of it.  Two knobs control the window:

| Setting | Default | What it does |
|---------|---------|-------------|
| `max_turns` | 20 | Hard ceiling on the number of back-and-forth exchanges kept. |
| `max_tokens` | unset | Optional soft cap on the total token count of the conversation. |

When either limit is exceeded, Mimir **drops the oldest complete turns**.  A turn is every message from a user message up to the next user message, so assistant tool-call messages and tool results are removed with their turn.  The system prompt is never removed, and the in-flight turn being answered is never trimmed away.

### Example

If `max_turns = 20` and you send 25 exchanges, the first 5 exchanges are deleted.  The system prompt plus the most recent 20 exchanges remain.

If token usage is known and the total exceeds `max_tokens`, Mimir drops oldest complete turns until the count is back under budget.

## Compaction: Summarise Instead of Silently Dropping

Before old turns are dropped, a background `session.compaction` job summarises them with the LLM, stores the summary on the session, and only then deletes the messages.  The summary is:

- Shown when you resume the conversation (`/history` in `mimir chat` prints `Earlier context: …`).
- Included in the session list API response.
- Fed back into the conversation context for future turns, so the model still knows the gist of what was discussed.

Compaction runs while you are idle (it never steals LLM capacity from your chat) and only when a session grows beyond the compaction window:

| Setting | Default | What it does |
|---------|---------|-------------|
| `context.compaction.enabled` | `true` | Master switch for background compaction. |
| `context.compaction.max_turns` | 15 | Keep the most recent this-many complete turns; older complete turns are summarised and removed. |

Keep `compaction.max_turns` below `context.max_turns` so compaction summarises turns before the hard trim would drop them. If the LLM call fails, the raw transcript of the compacted turns is kept instead (capped at 2000 characters), so a provider hiccup degrades the summary to a verbatim transcript rather than silently dropping the turns. Incognito sessions are never compacted because nothing is persisted.

## Configuring Limits

You can change the defaults in three ways (highest priority last):

1. **Edit `config/default.toml`** (affects new installations).
2. **Edit `~/.config/mimir/config.toml`** (user-wide override).
3. **Set environment variables** (temporary override):
   ```bash
   export MIMIR_CONTEXT_MAX_TOKENS=8192
   export MIMIR_CONTEXT_MAX_TURNS=50
   export MIMIR_CONTEXT_COMPACTION_ENABLED=true
   export MIMIR_CONTEXT_COMPACTION_MAX_TURNS=10
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
| Tool calls and tool results | Yes | Arguments and results can contain sensitive data; they are stored with their turn and trimmed with it. |
| Raw LLM SSE chunks | No | Only the final text and usage are kept. |
| Intermediate reasoning | No | If verbose reasoning is enabled, only the final reply is stored. |

## Best Practices

- Keep `max_turns` modest (10–30) for most use-cases; LLM quality degrades with very long contexts anyway.
- If you run a local model with a small context window, lower `max_tokens` accordingly.
- Move `db_path` to a fast SSD if you notice latency during heavy usage.
