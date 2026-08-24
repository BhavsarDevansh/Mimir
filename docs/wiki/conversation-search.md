# Conversation Search

Mimir can search your past conversations using full-text search powered by SQLite FTS5.

## What It Does

The `search_conversation_history` built-in tool lets the agent find relevant snippets from previous chats. Results are BM25-ranked and include contextual excerpts around each match, making it easy to surface prior decisions, facts, or context without re-asking.

## How the Agent Uses It

During a chat, if the agent needs to recall something from a prior conversation, it can invoke:

```json
{
  "query": "rust borrow checker",
  "limit": 5,
  "session_id": 42
}
```

- `query` — the terms to search for; every term must match, in any order (wrap the whole query in double quotes to require an exact phrase).
- `limit` — max results (default 5, max 20).
- `session_id` — optional; restricts search to a single conversation.

If `session_id` is omitted, all conversations are searched. Multi-word queries use AND semantics: `"check in time"` matches any message containing `check`, `in`, and `time` in any order, so a message like "time to check in" is found even though the exact phrase never appears. Hyphenated words are split like the FTS5 tokenizer splits them, so `check-in` and `check in` are equivalent.

## Result Format

Each result includes:

| Field | Description |
|-------|-------------|
| `session_id` | Which conversation the match came from |
| `role` | Message role (`user` or `assistant`) |
| `created_at` | UTC timestamp of the message |
| `snippet` | Contextual excerpt with `\u003c\u003c\u003c` and `\u003e\u003e\u003e` markers around the matched term |

## Example Query → Result Flow

**User:** "Remind me what we decided about the API timeout last week."

**Agent internally calls:**

```json
{ "query": "API timeout", "limit": 3 }
```

**Tool returns:**

```json
[
  {
    "session_id": 42,
    "role": "assistant",
    "created_at": "2026-06-05T14:23:00Z",
    "snippet": "...we agreed on a 30-second API timeout with exponential backoff..."
  }
]
```

**Agent:** "You decided on a 30-second API timeout with exponential backoff last Tuesday."

## Technical Notes

- Uses SQLite FTS5 virtual table (`messages_fts`) indexing `role` and `content`.
- Snippets are generated with `snippet(messages_fts, -1, '\u003c\u003c\u003c', '\u003e\u003e\u003e', '...', 30)` — a 30-token window on each side of the hit so matches inside long messages surface the surrounding answer.
- The FTS5 index is kept in sync via triggers on insert, update, and delete.
- Queries are tokenised and each token is quoted before building the FTS5 `MATCH` expression, so FTS5 operators (`AND`, `OR`, `NOT`, `*`, `-`, parentheses) cannot inject syntax; a query that is itself wrapped in double quotes keeps exact-phrase semantics.
