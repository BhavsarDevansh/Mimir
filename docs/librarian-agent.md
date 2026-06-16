# Librarian Agent

## Overview

The **Librarian Agent** is Mimir's background fact-extraction agent. After each
completed chat turn, it receives the full conversation transcript together with
the current knowledge-graph snapshot (condensed memory, user identity, and
recent related facts) and extracts structured facts into the knowledge graph.

It replaces the earlier fire-and-forget `spawn_fact_extraction` helper and is
the first implementation of the generic `Agent` / `AgentRuntime` framework.

## Responsibilities

- Receive a completed [`ConversationTurn`](../../mimir-core/src/conversation.rs).
- Resolve pronouns and disambiguate entities using the configured user identity.
- Detect contradictions against existing facts using the KB snapshot.
- Extract facts through the existing `remember` tool schema.
- Store facts with correct provenance, confidence, and status.
- Log results at `info` and errors at `warn`.

## Non-goals (out of scope for this iteration)

- Goal-directed KB research (e.g. planning a dinner party by cross-referencing
  multiple people's preferences and past events). This is a future agent that
  can invoke the Librarian as needed.
- Dedicated lightweight LLM for extraction. The Librarian currently uses the
  same LLM backend as the core agent.

## Architecture

```text
Chat Route
    |
    | after assistant response persisted
    v
LibrarianGoal { target_subject_id, topic, turn }
    |
    v
AgentRuntime.submit::<LibrarianAgent>(goal, LibrarianContext)
    |
    v
tokio::spawn -> LibrarianAgent::run(goal, ctx)
    |
    v
KnowledgeGraph::extract_facts_with_context(llm, turn, identity, memory)
    |
    v
KG entities/facts + audit log
```

## Public API

### Types

- `mimir_core::agents::Agent` — generic agent contract with an associated `Goal`
  and static `KIND`.
- `mimir_core::agents::AgentRuntime` — lightweight in-memory runtime that
  registers agents and dedupes `(kind, goal)` submissions.
- `mimir_core::conversation::ConversationTurn` — user message, assistant
  response, session id, and timestamp. The timestamp is recorded but excluded
  from equality and hashing so identical turns dedupe correctly.
- `mimir_core::identity::UserIdentity` — configured user's name and KG entity id.
- `mimir_knowledge::librarian::LibrarianAgent` — the concrete agent.
- `mimir_knowledge::librarian::LibrarianGoal` — `{ target_subject_id, topic, turn }`.
- `mimir_knowledge::librarian::LibrarianContext` — runtime context holding KG,
  LLM, identity, and optional condensed memory.
- `mimir_knowledge::KnowledgeGraph::extract_facts_with_context(...)` — rich-prompt
  extraction entrypoint.

### Deduping

The runtime dedupes by `(agent kind, goal hash)`. Two Librarian jobs with the
same `target_subject_id`, `topic`, and `ConversationTurn` content are considered
identical; only one runs at a time. Different topics or turn content run
independently.

## Data Flow

1. The chat route persists the assistant response in the conversation manager.
2. It builds a `ConversationTurn` and `LibrarianGoal` with topic
   `"chat-turn-extraction"`.
3. It builds a `LibrarianContext` with:
   - `Arc<KnowledgeGraph>`
   - `Arc<dyn LlmBackend>` (the core agent's LLM)
   - `UserIdentity`
   - optional condensed memory string
4. `AgentRuntime.submit::<LibrarianAgent>` queues the goal.
5. The runtime spawns a task that calls `LibrarianAgent::run`.
6. The agent calls `extract_facts_with_context`, which builds a prompt containing
   the transcript, identity, memory, and recent related facts.
7. The LLM emits facts via the `remember` tool; Rust validates and inserts them.

## Configuration

No dedicated configuration is required. The agent uses the same LLM settings as
the core agent (`[llm]` in `config.toml`).

## Testing

- Unit tests for `AgentRuntime` live in `mimir-core/src/agents/runtime.rs`.
- Integration tests for `LibrarianAgent` live in
  `mimir-knowledge/tests/librarian_agent.rs`.
- Server integration tests should assert that the chat route submits a goal and
  that the agent extracts facts in the background.

## Future Work

- Durable scheduling: move from the in-memory `AgentRuntime` to the durable
  `JobQueue` so goals survive daemon restarts.
- Goal-directed research: a higher-level agent that constructs `LibrarianGoal`s
  for specific research topics and synthesises findings for the core agent.
- Dedicated extraction LLM: optionally route extraction to a smaller, cheaper
  model separate from the main chat model.
