# Librarian Agent

## Overview

The **Librarian Agent** is Mimir's on-demand fact-extraction agent. When invoked, it receives the recent conversation as labelled messages together with the current core-facts block (the same condensed memory the core agent injects) and extracts structured facts into the knowledge graph.

It is the first implementation of the generic `Agent` / `AgentRuntime` framework.

> **Note (Issues #137, #386):** The Librarian is **no longer auto-invoked after every chat turn.** Learning is now hook-driven: the `remember.chat` background hook calls `KnowledgeGraph::extract_facts_with_context` directly after each non-incognito turn. The `LibrarianAgent` and `AgentRuntime` remain as a library API for future on-demand and bulk-extraction callers (e.g. a specialist research agent). The chat route no longer constructs or submits a `LibrarianGoal`.

## Responsibilities

- Receive a completed [`ConversationTurn`](../../mimir-core/src/conversation.rs).
- Extract only from user-authored messages, never from the assistant's own output (enforced by prompt labelling and source-discipline instructions).
- Check new facts against the core-facts block to avoid duplicating what is already known.
- Extract facts through the existing `remember_tool_schema` (the schema is retained for the extraction pipeline even though the `remember` tool was removed from the registry in #386).
- Store facts with correct provenance, confidence, and status.
- Log results at `info` and errors at `warn`.

## Non-goals (out of scope for this iteration)

- Goal-directed KB research (e.g. planning a dinner party by cross-referencing multiple people's preferences and past events). This is a future agent that can invoke the Librarian as needed.
- Dedicated lightweight LLM for extraction. The Librarian currently uses the same LLM backend as the core agent.

## Architecture

The Librarian is invoked on demand by a caller that constructs and submits a `LibrarianGoal` (a future specialist agent or bulk-import path). The chat route no longer drives it.

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
    | turn -> [User, Assistant] labelled messages
    v
KnowledgeGraph::extract_facts_with_context(llm, messages, memory)
    |
    v
KG entities/facts + audit log
```

## Public API

### Types

- `mimir_core::agents::Agent` — generic agent contract with an associated `Goal` and static `KIND`.
- `mimir_core::agents::AgentRuntime` — lightweight in-memory runtime that registers agents and dedupes `(kind, goal)` submissions.
- `mimir_core::conversation::ConversationTurn` — user message, assistant response, session id, and timestamp. The timestamp is recorded but excluded from equality and hashing so identical turns dedupe correctly.
- `mimir_core::conversation::ConversationMessage` / `MessageRole` — a labelled transcript message (`User` or `Assistant`). The prompt builder accepts a slice of these so the amount of context sent to the Librarian can grow in future without changing its signature.
- `mimir_knowledge::librarian::LibrarianAgent` — the concrete agent.
- `mimir_knowledge::librarian::LibrarianGoal` — `{ target_subject_id, topic, turn }`.
- `mimir_knowledge::librarian::LibrarianContext` — runtime context holding KG, LLM, and optional condensed memory (the core-facts block).
- `mimir_knowledge::KnowledgeGraph::extract_facts_with_context(...)` — rich-prompt extraction entrypoint.

### Prompt composition

`build_extraction_prompt` composes the Librarian's system prompt from:

1. **KG-focused base** (`build_base_prompt`) — extraction rules, the DB-driven Categorisation Guide, DB-derived predicate standards, list splitting, within-output deduplication, and the output contract. The guide renders the complete category tree with indentation so every seeded or user-added category remains selectable and deeper sub-categories cannot drift out of the prompt. The predicate standards are likewise rendered from the taxonomy's emit-eligible leaves (name plus DB description), so prompt and `remember` tool schema cannot drift apart (#598). Shared with the simple `extract_facts` path.
2. **Core-facts block** — the same `Personality::CORE_FACTS_HEADER` plus condensed memory the core agent injects, emitted only when non-empty. The user's identity (canonical name, entity details) is read from this block by the LLM, exactly as the core agent resolves identity — no separate identity parameter is passed (#139).
3. **Recent conversation** — the supplied messages rendered as labelled lines (`[User]: ...` / `[Assistant]: ...`) under `## Recent conversation`.
4. **Source discipline** — extract facts ONLY from `[User]` messages; never from `[Assistant]` messages (the LLM's own prior output to the user).
5. **Novelty check** — before emitting a fact, check it against the core-facts block; do not emit a fact that merely restates what is already known (exact duplicates are discarded by Rust regardless of classification), and use the `Correction` classification for corrections.

The transcript lives in the system prompt once; the user turn handed to the LLM is a short action instruction (no duplication).

### Deduping

The runtime dedupes by `(agent kind, goal hash)`. Two Librarian jobs with the same `target_subject_id`, `topic`, and `ConversationTurn` content are considered identical; only one runs at a time. Different topics or turn content run independently.

## Data Flow

1. An on-demand caller builds a `ConversationTurn` and a `LibrarianGoal` (the topic is caller-defined; the historical `"chat-turn-extraction"` topic is no longer produced by the chat route).
2. It builds a `LibrarianContext` with:
   - `Arc<KnowledgeGraph>`
   - `Arc<dyn LlmBackend>` (the core agent's LLM)
   - optional condensed memory string
3. `AgentRuntime.submit::<LibrarianAgent>` queues the goal.
4. The runtime spawns a task that calls `LibrarianAgent::run`.
5. The agent converts the turn into `[User, Assistant]` messages and calls `extract_facts_with_context`, which builds a prompt containing the core-facts block and the labelled recent conversation.
6. The LLM emits facts via the `remember_tool_schema`; Rust validates and inserts them.

For ordinary chat, learning bypasses the `AgentRuntime` entirely: the `remember.chat` hook handler calls `extract_facts_with_context` directly, and Rust's `process_remember_output` applies the same policy (confidence, overwrite, sensitive gating) without a second LLM call.

## Configuration

No dedicated configuration is required. The agent uses the same LLM settings as the core agent (`[llm]` in `config.toml`).

## Testing

- Unit tests for `AgentRuntime` live in `mimir-core/src/agents/runtime.rs`.
- Integration tests for `LibrarianAgent` (invoked explicitly) live in `mimir-knowledge/tests/librarian_agent.rs`.
- Server integration tests in `mimir-server/tests/chat_learning_tests.rs` assert the new model: non-incognito blocking and streaming turns enqueue the `remember.chat` hook and persist facts, and incognito turns never enqueue any hook and write no facts.

## Future Work

- Durable scheduling: move from the in-memory `AgentRuntime` to the durable `JobQueue` so goals survive daemon restarts.
- Goal-directed research: a higher-level agent that constructs `LibrarianGoal`s for specific research topics and synthesises findings for the core agent.
- Dedicated extraction LLM: optionally route extraction to a smaller, cheaper model separate from the main chat model.
