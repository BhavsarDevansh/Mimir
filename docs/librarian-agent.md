# Librarian Agent

## Overview

The **Librarian Agent** is Mimir's on-demand fact-extraction agent. When invoked,
it receives the full conversation transcript together with the current
knowledge-graph snapshot (condensed memory, user identity, and recent related
facts) and extracts structured facts into the knowledge graph.

It is the first implementation of the generic `Agent` / `AgentRuntime` framework.

> **Note (Issue #137):** The Librarian is **no longer auto-invoked after every
> chat turn.** Learning is now LLM-orchestrated: the conversational LLM calls the
> `remember` tool inline to persist facts. The Librarian and
> `KnowledgeGraph::extract_facts_with_context` remain as a library API for future
> on-demand and bulk-extraction callers (e.g. a specialist research agent). The
> chat route no longer constructs or submits a `LibrarianGoal`.

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

The Librarian is invoked on demand by a caller that constructs and submits a
`LibrarianGoal` (a future specialist agent or bulk-import path). The chat route
no longer drives it.

```text
On-demand caller (future agent / bulk import)
    |
    | constructs LibrarianGoal { target_subject_id, topic, turn }
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

1. An on-demand caller builds a `ConversationTurn` and a `LibrarianGoal`
   (the topic is caller-defined; the historical `"chat-turn-extraction"` topic is
   no longer produced by the chat route).
2. It builds a `LibrarianContext` with:
   - `Arc<KnowledgeGraph>`
   - `Arc<dyn LlmBackend>` (the core agent's LLM)
   - `UserIdentity`
   - optional condensed memory string
3. `AgentRuntime.submit::<LibrarianAgent>` queues the goal.
4. The runtime spawns a task that calls `LibrarianAgent::run`.
5. The agent calls `extract_facts_with_context`, which builds a prompt containing
   the transcript, identity, memory, and recent related facts.
6. The LLM emits facts via the `remember` tool; Rust validates and inserts them.

For ordinary chat, learning bypasses this agent entirely: the LLM calls
`remember` inline and Rust's `process_remember_output` applies the same policy
(confidence, overwrite, sensitive gating) without a second LLM call.

## Configuration

No dedicated configuration is required. The agent uses the same LLM settings as
the core agent (`[llm]` in `config.toml`).

## Testing

- Unit tests for `AgentRuntime` live in `mimir-core/src/agents/runtime.rs`.
- Integration tests for `LibrarianAgent` (invoked explicitly) live in
  `mimir-knowledge/tests/librarian_agent.rs`.
- Server integration tests in `mimir-server/src/lib.rs` assert the new model:
  `test_chitchat_does_not_trigger_background_learning` verifies that a chitchat
  turn makes no background extraction LLM call, and
  `test_chat_extracts_facts_after_response` verifies that an inline `remember`
  tool call persists facts.

## Future Work

- Durable scheduling: move from the in-memory `AgentRuntime` to the durable
  `JobQueue` so goals survive daemon restarts.
- Goal-directed research: a higher-level agent that constructs `LibrarianGoal`s
  for specific research topics and synthesises findings for the core agent.
- Dedicated extraction LLM: optionally route extraction to a smaller, cheaper
  model separate from the main chat model.
