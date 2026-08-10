# Retrieval Agent

## Overview

The **Retrieval Agent** is an internal, ephemeral LLM session that Mimir's main agent launches when it needs structured context from the knowledge graph or conversation history. It is the implementation of Issue #128 (Agentic Pre-Response Context Retrieval).

## Architecture

```text
Main LLM
    ↓  calls retrieve_context tool
RetrieveContextTool
    ↓  spawns
RetrievalAgent (ephemeral session)
    ↓  runs up to 25 rounds
    ├─ kg_query (facts about entities)
    ├─ kg_related (graph traversal)
    ├─ kg_search (FTS5 entity search)
    ├─ search_conversation_history (FTS5 conversation search)
    └─ finish_retrieval (termination signal)
    ↓  returns
RetrievedContext (JSON)
    ↓  passed back to
Main LLM via ToolOutput
```

## Key Design Decisions

- **Ephemeral session**: The retrieval agent maintains its own `Vec<Message>` conversation. It is never persisted to `ContextManager`.
- **Private ToolRegistry**: Only retrieval tools are registered. External APIs and side-effect tools are excluded.
- **25-round hard limit**: Circuit breaker to prevent runaway loops.
- **Structured output**: `RetrievedContext` contains `entities`, `relations`, `conversation_snippets`, and `finish_reason`.
- **Deduplication**: Duplicate entities, facts, relations, and conversation snippets are deduplicated during accumulation.
- **Error resilience**: Tool failures are reported back to the retrieval LLM as error messages, allowing it to decide whether to retry or finish.

## Data Types

### RetrievedContext

```rust
pub struct RetrievedContext {
    pub entities: Vec<RetrievedEntity>,
    pub relations: Vec<RetrievedRelation>,
    pub conversation_snippets: Vec<ConversationSnippet>,
    pub finish_reason: Option<String>,
    pub rounds_used: u16,
}
```

## Files

- `mimir-knowledge/src/retrieval/agent.rs` — `RetrievalAgent` implementation
- `mimir-knowledge/src/retrieval/types.rs` — `RetrievedContext` and related types
- `mimir-knowledge/src/tools/retrieve_context.rs` — `RetrieveContextTool` exposed to main LLM
- `mimir-knowledge/tests/retrieval_tests.rs` — unit tests

## Integration

The `retrieve_context` tool is registered in `mimir-server/src/state/` alongside other KG tools. The main LLM's tool loop automatically makes it available when the system prompt encourages thorough investigation.
