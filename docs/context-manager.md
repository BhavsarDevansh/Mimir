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
    compacted_at TEXT,
    user_key TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_user_key
ON sessions(user_key) WHERE user_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls TEXT,
    tool_call_id TEXT,
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
- **Search semantics:** `search_messages` tokenises the query (splitting on any run of non-alphanumeric characters, mirroring the FTS5 unicode61 tokenizer) and AND-combines the quoted tokens, so every term must match in any order and hyphenated forms like `check-in` match `check in`. A query wrapped in double quotes keeps exact-phrase semantics. Each token is quoted before building the `MATCH` expression, so FTS5 operators cannot inject syntax. Snippets use a 30-token context window on each side of the hit (issue #493). The session-filtered and unfiltered paths share a single `QueryBuilder`-assembled statement (issue #500): the SELECT list, snippet call, join, and ordering exist once, and the optional `m.session_id = ?` clause is appended only when a session filter is supplied, so the query shape can no longer drift between the two paths.
- **Cascading delete** ensures `DELETE FROM sessions` removes all messages.
- `summary` holds the LLM summary of compacted turns (issue #279); it is written by the compaction pipeline, injected into `export_messages` as a clearly labelled user-role context block, and surfaced by the session list / resume flow.
- `compacted_at` is an RFC 3339 timestamp set to the last summarised message's timestamp; `get_messages_after_compaction` (`created_at >= compacted_at`) returns exactly the retained window.
- `user_key` is the nullable OpenAI `user` conversation key (issue #388); the partial unique index allows exactly one session per non-NULL key while leaving native sessions unconstrained.
- `tool_calls` stores the assistant's tool-call array as JSON for `export_messages` round-tripping, and `tool_call_id` links each `tool`-role result to the call it answers (issue #388).

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
   - If all non-system messages have known `token_count`, delete the oldest complete turns until the sum is under budget. `tool`-role messages never carry token counts (usage is attributed only to user and assistant messages), so they are excluded from the unknown-count probe and do not force the conservative fallback.
   - If some messages lack `token_count` (e.g. streaming without usage), fall back to deleting the oldest complete turns until the turn count is under `max_turns / 2` (conservative).
   - The system prompt is **never** deleted.

A turn spans from a user message up to (but excluding) the next user message, so assistant tool-call messages and tool results are removed with their turn instead of being orphaned (issue #388).  The in-flight turn being answered — its user message was just persisted and has no final assistant reply yet (an assistant tool-call message still awaits the client's tool results) — is never trimmed away before the LLM call.

## Compaction

`trim_to_budget` alone silently discards context, so a background compaction pass summarises the oldest complete turns before trimming would drop them (issue #279). Because the background hook is debounced and idle-gated, the request paths also compact synchronously when a burst reaches the hard `max_turns` ceiling, so the turns the trim is about to delete are always summarised first (PR #505 review).

### Pipeline

1. **`compaction_candidates(session_id, max_turns)`** (deterministic, DB-only) splits the session's non-system rows into complete turns via the shared `split_complete_turns` helper in `trim.rs` (same turn-boundary and in-flight-final-turn rules as trimming), and returns the oldest complete turns beyond `max_turns` plus the session's existing summary.
2. **`SessionCompactor::compact_session(session_id)`** renders the candidates as a labelled transcript, asks the LLM for a concise summary (folding in any previous summary), then commits via `apply_compaction`. If the LLM call fails or returns nothing, the transcript itself is stored (capped at `MAX_COMPACTION_SUMMARY_CHARS`, 2000) so the turns are never silently discarded.
3. **`apply_compaction(session_id, summary, compacted_at, delete_ids)`** writes `sessions.summary`, advances `compacted_at`, and deletes the summarised messages in one transaction — a single session-scoped `DELETE ... WHERE id IN (...)` commits atomically with the summary write, so a failure part-way can never leave a new summary alongside summarised messages that still exist (PR #505 review). Re-applying an already-deleted batch is a no-op for the deletes.

### Triggering and config

The `session.compaction` hook (registered in `init_hook_engine`) fires on `TurnCompleted` with per-session `SingularLastWins` debounce and `IdleGated` dispatch, mirroring `remember.chat` — incognito turns never trigger it because nothing is persisted. It is registered only when `context.compaction.enabled` is true; `context.compaction.max_turns` (default 15) is the window. The window must stay below `context.max_turns` (default 20) so compaction summarises turns before the synchronous trim deletes them; `Config::normalise` clamps an equal or inverted window to `max_turns - 1` after TOML and environment overrides are applied (PR #505 review). The trim remains the hard safety ceiling when the user races ahead of the idle-gated job: on both request paths (`/chat` and the OpenAI surface), `AppState::compact_before_hard_trim` runs the compaction synchronously before `trim_to_budget` when the session is already over `max_turns`, so turns are summarised into `sessions.summary` before any are dropped.

### Read paths

- `export_messages` injects the summary as a clearly labelled user-role context block right after the system prompt, so the model sees the gist of compacted turns without promoting potentially user-influenced summary text to system-authority context (PR #505 review).
- `list_sessions` / `load_session` expose `summary` on `SessionSummary` / `Session`; the server surfaces it on `GET /sessions` and `GET /sessions/{id}/messages`, and the REPL `/history` resume prints `Earlier context: …`.

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
    pub async fn user_turn_count(&self, session_id: i64) -> Result<i64, ContextError>;
    pub async fn trim_to_budget(
        &self, session_id: i64, max_tokens: Option<u32>, max_turns: u16
    ) -> Result<(), ContextError>;
    pub async fn export_messages(&self, session_id: i64
    ) -> Result<Vec<Message>, ContextError>;
    pub async fn export_conversation(&self, session_id: i64
    ) -> Result<ConversationExport, ContextError>;
    pub async fn max_message_id(&self, session_id: i64) -> Result<i64, ContextError>;
    pub async fn delete_messages_after(
        &self, session_id: i64, after_id: i64
    ) -> Result<u64, ContextError>;
    pub async fn delete_session(&self, session_id: i64) -> Result<(), ContextError>;
    pub async fn search_messages(
        &self, query: &str, limit: usize, session_id: Option<i64>
    ) -> Result<Vec<MessageSearchResult>, ContextError>;
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ContextError>;
    pub async fn get_messages_after_compaction(
        &self, session_id: i64
    ) -> Result<Vec<ContextMessage>, ContextError>;
    pub async fn load_session(&self, session_id: i64) -> Result<Session, ContextError>;
    pub async fn compaction_candidates(
        &self, session_id: i64, max_turns: u16
    ) -> Result<Option<CompactionCandidates>, ContextError>;
    pub async fn apply_compaction(
        &self, session_id: i64, summary: &str, compacted_at: DateTime<Utc>, delete_ids: &[i64]
    ) -> Result<(), ContextError>;
}

pub struct SessionCompactor { ... }
impl SessionCompactor {
    pub fn new(context: Arc<ContextManager>, llm: Arc<dyn LlmBackend>, max_turns: u16) -> Self;
    pub async fn compact_session(&self, session_id: i64)
        -> Result<Option<CompactionOutcome>, ContextError>;
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
