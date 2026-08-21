# Learning Orchestration

> **Issues:** #137, #386
>
> **Phase:** 2 — Knowledge Graph / Core Agent
>
> **Version:** 0.131.0

## Overview

Mimir learns facts about the user from conversation. Issue #137 asked for a Rust intent detector that classifies every utterance before the LLM sees it. This document describes the designs Mimir adopted instead, and why.

The short version: **NLU is the LLM's job, and orchestration is deterministic Rust.** Mimir does not run a parallel hand-rolled intent classifier next to a capable LLM. The LLM decides *what* is worth learning; Rust decides *when* learning runs and *how* facts are validated and inserted.

## Why not a Rust intent classifier

A regex/keyword intent classifier is brittle and largely redundant when a capable LLM is already in the loop. Utterances like "My favourite colour is blue", "Blue is nice", and "I think I kind of like blue" are easy for a model and hard for a regex, and the model already understands them for free. Building a second NLU pipeline beside the LLM duplicates work the LLM already does.

## History: tool-call learning (#137), then hooks (#386)

The first fix for the "passive chatbot" bug retired the unconditional Librarian Agent that ran after *every* non-incognito chat turn. Learning happened only when the conversational LLM called the `remember` tool inline while composing its reply, and retrieval stayed LLM-driven via `retrieve_context`. Intent — "should I learn?", "should I retrieve?" — was an emergent property of tool selection.

Issue #386 replaced tool-call remembering with the server-side hooks engine. The `remember` tool was removed from the registry and the system prompt, and learning now runs as a deterministic background hook (`remember.chat`) triggered by turn completion. This removes the prompt-injection path where a user could steer the model into or out of calling `remember`, and it makes learning work for OpenAI-compatible remote clients that never call tools (issue #388). The accepted cost is an occasional trivial extraction (a lone "hello" after the debounce window).

## The boundary: LLM extracts, Rust enforces

This split keeps the deterministic guarantees the project requires (see `AGENTS.md`: "Changing the underlying LLM model should never require rewriting application code"):

- **The LLM extracts facts** — the per-fact `classification` (`Explicit` / `Casual` / `Correction`), `is_sensitive`, and `correction_scope` come from the extraction call.
- **Rust enforces policy** — `process_remember_output` / `process_fact_batch` map classification to confidence, apply the overwrite/coexistence matrix, run the sensitive-confirmation gate, resolve predicates through the alias table, and insert facts. The model cannot set confidence, decide overwrite semantics, or bypass sensitive confirmation. The contract is stable in Rust, so swapping models changes quality, not correctness.

The overwrite/coexistence matrix from `VISION/02-Knowledge-Graph/Learning-Modes.md` is unchanged and enforced in Rust. The shipped policy pins Casual confidence at exactly `0.30` (`mimir-knowledge/src/confidence.rs`), superseding the `0.2–0.4` design range in the VISION doc:

| New ↓ / Existing → | Explicit (1.0) | Casual (0.30) | Inferred |
|--------------------|----------------|---------------|----------|
| **Explicit (1.0)** | Overwrite      | Overwrite     | Overwrite |
| **Casual (0.30)**   | Coexist        | Coexist       | Coexist  |

## What changed in code

- `mimir-core/src/hooks/` — the hooks engine: typed triggers, queue policies (`Multiple`, `SingularFirstWins`, `SingularLastWins` with debounce), key scopes, idle gates, retry, and the durable `JobQueue` dispatch loop.
- `mimir-server/src/state/hooks.rs` — the `remember.chat` and `memory.condensation` handlers; `mimir-server/src/state/builder.rs` registers the hooks.
- `mimir-server/src/routes/chat.rs` — both the blocking and streaming paths trigger `TurnCompleted` after the assistant message is persisted, non-incognito sessions only.
- `mimir-knowledge/src/tools/remember.rs` — deleted; the `remember` tool is no longer registered or exported. The `remember_tool_schema` remains as the extraction schema for the hook pipeline.
- `mimir-core/src/personality.rs` — the "call `remember`" operating directive was removed.

## What was kept

The `LibrarianAgent`, `LibrarianGoal` / `LibrarianContext`, and `KnowledgeGraph::extract_facts_with_context` are retained as a library API — the `remember.chat` hook handler now calls `extract_facts_with_context` directly. `retrieve_context` and the KG query tools stay LLM tools.

## Testing

- `mimir-core/src/hooks/tests.rs` — unit tests for each queue policy, key scope, debounce window, idle gating, retry, and shutdown.
- `mimir-server/tests/chat_learning_tests.rs` — non-incognito blocking and streaming turns enqueue the hook and persist facts; incognito turns never enqueue any hook and write no facts.
- `mimir-server/tests/kb_query_tests.rs` — the `remember` tool is absent from the registry and the OpenAI export.
- `mimir-knowledge/tests/librarian_agent.rs` — the Librarian still works when invoked explicitly (library API intact).

## Future

A `request_deep_research` orchestration tool (spawning the Reasoning Engine, Phase 4) will follow the same pattern: the LLM decides to invoke it; Rust owns the agent-spawn semantics. The contract stays stable across model changes.
