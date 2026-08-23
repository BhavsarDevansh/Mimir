# Conversation Context Manager

## Overview

The context manager (`mimir-core::context`) maintains multi-turn dialogue state across interactions.  It persists sessions and messages to SQLite via **SQLx**, supports token-aware trimming using actual token counts returned by the LLM API, and exports conversation history for LLM requests and audit/logging.

## Design Decisions

### SQLx over rusqlite

SQLx was chosen because it is async-native, provides built-in connection pooling (`SqlitePool`), supports `chrono`/`uuid` type mappings out of the box, and uses parameterised queries by default.  This keeps the storage layer consistent with the rest of the async stack (tokio, reqwest, axum).

### Runtime queries (not `query!` macros)

Phase 1 uses runtime `sqlx::query` and `sqlx::query_as` to avoid the extra complexity of `sqlx-cli`, offline-mode schema files, and compile-time checking. The code is still type-safe via `query_as::<_, Struct>`.  If query coverage grows significantly we can migrate to macros later without changing the public API.

### Session cache

`ContextManager` keeps an in-memory `Arc<tokio::sync::Mutex<HashSet<String>>>` that tracks known session IDs.  This avoids a DB round-trip on every `ensure_session_exists` check; the cache is updated on `create_session` and `delete_session`.  `tokio::sync::Mutex` is used because the guard may be held across `.await` points.

### No in-memory LRU cache

Sessions are tiny (≈20 messages).  SQLite with WAL mode is fast enough for local-first use.  An in-memory cache can be added later if profiling shows a bottleneck.

## SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    system_prompt TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
    summary TEXT,
    compacted_at TEXT
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    token_count INTEGER
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_at);

-- FTS5 virtual table for full-text search over messages
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    role, content, content='messages', content_rowid='id'
);
```

- **WAL mode** is enabled on every connection for better concurrency.
- **Cascading delete** ensures `DELETE FROM sessions` removes all messages.
- The `summary` column is reserved for Phase 2 summarisation work.
- `compacted_at` is an RFC 3339 timestamp marking the start of the retained message window. Messages before this point were compacted/summarised in Phase 2.

## Token Attribution

`record_usage` expects **per-call delta values** — the number of tokens attributable to this single request:

| Token type | Attribution rule |
|-----------|-----------------|
| `prompt_tokens` | Per-call delta → most recent user message. |
| `completion_tokens` | Full amount per call → most recent assistant message. |

Deltas are added to the stored cumulative totals on the session row and to the `token_count` of the respective most recent message.  Zero or negative deltas are ignored so cumulative totals never decrease.

## Trimming Algorithm

`trim_to_budget(session_id, max_tokens, max_turns)` operates in two phases:

1. **Turn cap (hard):** Count user messages (each user message starts a turn, so tool messages belong to their turn).  If the count exceeds `max_turns`, delete the oldest complete turns until the count is under the limit.

2. **Token budget (soft):** If `SUM(token_count)` for the session exceeds `max_tokens`:
   - If all non-system messages have known `token_count`, delete oldest complete turns until the sum is under budget.
   - If some messages lack `token_count` (e.g. streaming without usage), fall back to deleting oldest complete turns until the turn count is under `max_turns / 2` (conservative).
   - The system prompt is **never** deleted.

A turn spans from a user message up to (but excluding) the next user message, so assistant tool-call messages and tool results are removed with their turn instead of being orphaned (issue #388).  The in-flight turn being answered — its user message was just persisted and has no assistant reply yet — is never trimmed away before the LLM call.

## API Surface

```rust
pub struct ContextManager { ... }

impl ContextManager {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, ContextError>;
    pub async fn create_session(&self, system_prompt: impl Into<String>
    ) -> Result<i64, ContextError>;
    pub async fn add_user_message(&self, session_id: i64, content: impl Into<String>
    ) -> Result<(), ContextError>;
    pub async fn add_assistant_message(&self, session_id: i64, content: impl Into<String>
    ) -> Result<(), ContextError>;
    pub async fn record_usage(
        &self, session_id: i64, prompt_tokens: u32, completion_tokens: u32
    ) -> Result<(), ContextError>;
    pub async fn trim_to_budget(
        &self, session_id: i64, max_tokens: Option<u32>, max_turns: u16
    ) -> Result<(), ContextError>;
    pub async fn export_messages(&self, session_id: i64
    ) -> Result<Vec<Message>, ContextError>;
    pub async fn export_conversation(&self, session_id: i64
    ) -> Result<ConversationExport, ContextError>;
    pub async fn delete_session(&self, session_id: i64) -> Result<(), ContextError>;
    pub async fn search_messages(
        &self, query: &str, limit: usize, session_id: Option<i64>
    ) -> Result<Vec<MessageSearchResult>, ContextError>;
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ContextError>;
    pub async fn get_messages_after_compaction(
        &self, session_id: &str
    ) -> Result<Vec<ContextMessage>, ContextError>;
}
```

## Integration Pattern

### Non-streaming

```rust
let messages = ctx_mgr.export_messages(&session_id).await?;
let (response, usage) = llm_client.chat(messages).await?;
ctx_mgr.add_assistant_message(&session_id, &response).await?;
ctx_mgr.record_usage(&session_id, usage.prompt_tokens, usage.completion_tokens).await?;
let budget = config.context.max_tokens.or_else(|| {
    // Query the endpoint for the model's context window.
    llm_client.fetch_model_context_window().ok()??
});
ctx_mgr.trim_to_budget(&session_id, budget, config.context.max_turns).await?;
```

### Streaming

```rust
let messages = ctx_mgr.export_messages(&session_id).await?;
let mut stream = llm_client.chat_stream_with_usage(messages).await?;
let mut response_text = String::new();
let mut usage: Option<Usage> = None;
while let Some(item) = stream.next().await {
    match item? {
        StreamItem::Text(chunk) => { response_text.push_str(&chunk); }
        StreamItem::Usage(u) => { usage = Some(u); }
    }
}
ctx_mgr.add_assistant_message(&session_id, &response_text).await?;
if let Some(u) = usage {
    ctx_mgr.record_usage(
        &session_id, u.prompt_tokens, u.completion_tokens
    ).await?;
}
let budget = config.context.max_tokens.or_else(|| {
    llm_client.fetch_model_context_window().ok()??
});
ctx_mgr.trim_to_budget(
    &session_id, budget, config.context.max_turns
).await?;
```
