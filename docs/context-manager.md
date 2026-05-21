# Conversation Context Manager

## Overview

The context manager (`mimir-core::context`) maintains multi-turn dialogue state
across interactions.  It persists sessions and messages to SQLite via **SQLx**,
supports token-aware trimming using actual token counts returned by the LLM API,
and exports conversation history for LLM requests and audit/logging.

## Design Decisions

### SQLx over rusqlite

SQLx was chosen because it is async-native, provides built-in connection pooling
(`SqlitePool`), supports `chrono`/`uuid` type mappings out of the box, and uses
parameterised queries by default.  This keeps the storage layer consistent with
the rest of the async stack (tokio, reqwest, axum).

### Runtime queries (not `query!` macros)

Phase 1 uses runtime `sqlx::query` and `sqlx::query_as` to avoid the extra
complexity of `sqlx-cli`, offline-mode schema files, and compile-time checking.
The code is still type-safe via `query_as::<_, Struct>`.  If query coverage grows
significantly we can migrate to macros later without changing the public API.

### No `std::sync::Mutex`

A synchronous `Mutex` cannot be held across `.await` points in async Rust.
Instead, `ContextManager` holds `Arc<SqlitePool>` directly — the pool itself is
`Clone + Send + Sync` and provides safe concurrent access without explicit
locking.

### No in-memory LRU cache

Sessions are tiny (≈20 messages).  SQLite with WAL mode is fast enough for
local-first use.  An in-memory cache can be added later if profiling shows a
bottleneck.

## SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    system_prompt TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
    summary TEXT
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    token_count INTEGER
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_at);
```

- **WAL mode** is enabled on every connection for better concurrency.
- **Cascading delete** ensures `DELETE FROM sessions` removes all messages.
- The `summary` column is reserved for Phase 2 summarisation work.

## Token Attribution

The OpenAI-compatible API returns **total** `prompt_tokens` for the entire
request (system prompt + full history + current user message).  We derive
approximate per-message counts:

| Recording | `prompt_tokens` attribution | `completion_tokens` attribution |
|-----------|----------------------------|--------------------------------|
| First     | Full amount → most recent user message | Full amount → most recent assistant message |
| Subsequent | `delta = new_prompt - previous_cumulative` → most recent user message | Full amount → most recent assistant message |

These approximations are stored on each message row as `token_count` and are
used by the trimming algorithm.

## Trimming Algorithm

`trim_to_budget(session_id, max_tokens, max_turns)` operates in two phases:

1. **Turn cap (hard):** Count non-system messages.  If the count exceeds
   `max_turns * 2`, delete the oldest complete `(user, assistant)` pairs until
   the count is under the limit.

2. **Token budget (soft):** If `SUM(token_count)` for the session exceeds
   `max_tokens`:
   - If all non-system messages have known `token_count`, delete oldest pairs
     until the sum is under budget.
   - If some messages lack `token_count` (e.g. streaming without usage), fall
     back to deleting oldest pairs until the pair count is under `max_turns / 2`
     (conservative).
   - The system prompt is **never** deleted.

## API Surface

```rust
pub struct ContextManager { ... }

impl ContextManager {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, ContextError>;
    pub async fn create_session(&self, system_prompt: impl Into<String>
    ) -> Result<String, ContextError>;
    pub async fn add_user_message(&self, session_id: &str, content: impl Into<String>
    ) -> Result<(), ContextError>;
    pub async fn add_assistant_message(&self, session_id: &str, content: impl Into<String>
    ) -> Result<(), ContextError>;
    pub async fn record_usage(
        &self, session_id: &str, prompt_tokens: u32, completion_tokens: u32
    ) -> Result<(), ContextError>;
    pub async fn trim_to_budget(
        &self, session_id: &str, max_tokens: u32, max_turns: u16
    ) -> Result<(), ContextError>;
    pub async fn export_messages(&self, session_id: &str
    ) -> Result<Vec<Message>, ContextError>;
    pub async fn export_conversation(&self, session_id: &str
    ) -> Result<ConversationExport, ContextError>;
    pub async fn delete_session(&self, session_id: &str) -> Result<(), ContextError>;
}
```

## Integration Pattern

### Non-streaming

```rust
let messages = ctx_mgr.export_messages(&session_id).await?;
let (response, usage) = llm_client.chat(messages).await?;
ctx_mgr.add_assistant_message(&session_id, &response).await?;
ctx_mgr.record_usage(&session_id, usage.prompt_tokens, usage.completion_tokens).await?;
ctx_mgr.trim_to_budget(&session_id, config.context.max_tokens, config.context.max_turns).await?;
```

### Streaming

```rust
let messages = ctx_mgr.export_messages(&session_id).await?;
let mut stream = llm_client.chat_stream_with_usage(messages).await?;
let mut response_text = String::new();
while let Some(item) = stream.next().await {
    match item? {
        StreamItem::Text(chunk) => { response_text.push_str(&chunk); }
        StreamItem::Usage(usage) => {
            ctx_mgr.record_usage(
                &session_id, usage.prompt_tokens, usage.completion_tokens
            ).await?;
        }
    }
}
ctx_mgr.add_assistant_message(&session_id, &response_text).await?;
ctx_mgr.trim_to_budget(&session_id, config.context.max_tokens, config.context.max_turns).await?;
```
