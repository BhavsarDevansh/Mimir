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

To keep LLM requests fast and within token limits, Mimir keeps a bounded window of conversation history; the oldest turns that fall out of it are summarised before deletion when compaction runs, and otherwise dropped.  Two knobs control the window:

| Setting | Default | What it does |
|---------|---------|-------------|
| `max_turns` | 20 | Hard ceiling on the number of back-and-forth exchanges kept. |
| `max_tokens` | unset | Optional soft cap on the total token count of the conversation. |

When either limit is exceeded, Mimir **drops the oldest complete turns**.  A turn is every message from a user message up to the next user message, so assistant tool-call messages and tool results are removed with their turn.  The system prompt is never removed, and the in-flight turn being answered is never trimmed away.  Compaction summarises turns before deletion only in two cases: the background job runs while you are idle and the session is beyond the compaction window, and the request path compacts synchronously when a burst reaches the hard `max_turns` ceiling.  Token-budget (`max_tokens`) trimming is never preceded by compaction and drops the oldest turns without a summary.

### Example

If `max_turns = 20` and you send 25 exchanges, the first 5 exchanges are deleted.  The system prompt plus the most recent 20 exchanges remain.

If token usage is known and the total exceeds `max_tokens`, Mimir drops oldest complete turns until the count is back under budget.

## Compaction: Summarise Instead of Silently Dropping

A background `session.compaction` job summarises old turns with the LLM, stores the summary on the session, and only then deletes the messages.  The job runs while you are idle, so a long uninterrupted burst of messages can still reach the hard `max_turns` ceiling before the job runs; in that case the request path compacts the excess turns synchronously before dropping them, so the oldest turns are still summarised into the session (your reply simply waits for that one extra summarisation call).  The summary is:

- Shown when you resume the conversation (`/history` in `mimir chat` prints `Earlier context: …`).
- Included in the session list API response.
- Fed back into the conversation context for future turns, so the model still knows the gist of what was discussed.

Compaction normally runs while you are idle (it never steals LLM capacity from your chat) and only when a session grows beyond the compaction window; the synchronous run at the hard ceiling is the one exception, because dropping turns without a summary would lose context:

| Setting | Default | What it does |
|---------|---------|-------------|
| `context.compaction.enabled` | `true` | Master switch for background compaction. |
| `context.compaction.max_turns` | 15 | Keep the most recent this-many complete turns; older complete turns are summarised and removed. |

Keep `compaction.max_turns` below `context.max_turns` so compaction summarises turns before the hard trim would drop them; if the two are equal or inverted, Mimir clamps the compaction window to one turn below the hard ceiling at startup and on config reload, so the guarantee holds even with custom settings. The clamp and the synchronous compact-before-trim path use the live (reloaded) values, but the background job's window and enablement are fixed at daemon startup, so changing them takes effect for the background job only after a restart. If the LLM call fails, the raw transcript of the compacted turns is kept instead (capped at 2000 characters), so a provider hiccup degrades the summary to a verbatim transcript rather than silently dropping the turns. Incognito sessions are never compacted because nothing is persisted.

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
- **SQLite connection settings** use WAL mode for safe concurrent reads and writes, `synchronous=NORMAL` and a 10,000-page cache for faster chat writes; WAL preserves database consistency, but a power loss can roll back recent committed chat writes.

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
