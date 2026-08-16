# Learning Orchestration

> **Issue:** #137
>
> **Phase:** 2 — Knowledge Graph / Core Agent
>
> **Version:** 0.51.0

## Overview

Mimir learns facts about the user from conversation. Issue #137 asked for a Rust intent detector that classifies every utterance before the LLM sees it. This document describes the design Mimir adopted instead, and why.

The short version: **NLU is the LLM's job, and orchestration emerges from structured tool selection.** Mimir does not run a parallel hand-rolled intent classifier next to a capable LLM. The LLM decides *whether* to learn and *whether* to retrieve by calling tools; Rust decides *how* and *what is allowed* when a tool fires.

## Why not a Rust intent classifier

A regex/keyword intent classifier is brittle and largely redundant when a capable LLM is already in the loop. Utterances like "My favourite colour is blue", "Blue is nice", and "I think I kind of like blue" are easy for a model and hard for a regex, and the model already understands them for free. Building a second NLU pipeline beside the LLM duplicates work the LLM already does.

## The model

Two tools already existed and were already LLM-driven:

- `retrieve_context` — pre-response knowledge-graph retrieval (Issue #128).
- `remember` — write structured facts to the knowledge graph.

The actual "passive chatbot" bug was that the **Librarian Agent** ran unconditionally after *every* non-incognito chat turn, firing a second background extraction LLM call regardless of whether the turn was chitchat or a real assertion. So Mimir learned from "hello" and double-extracted when the LLM had already called `remember`.

The fix retires the unconditional Librarian. Learning now happens only when the conversational LLM calls `remember` inline while composing its reply. Retrieval stays LLM-driven via `retrieve_context`. Intent — "should I learn?", "should I retrieve?" — is an emergent property of tool selection, not a pre-classification step.

## The boundary: LLM decides, Rust enforces

This split keeps the deterministic guarantees the project requires (see `AGENTS.md`: "Changing the underlying LLM model should never require rewriting application code"):

- **The LLM decides intent** — whether to call `remember` / `retrieve_context`, and the per-fact `classification` (`Explicit` / `Casual` / `Correction`), `is_sensitive`, and `correction_scope`.
- **Rust enforces policy** — `process_remember_output` / `process_fact_batch` map classification to confidence, apply the overwrite/coexistence matrix, run the sensitive-confirmation gate, resolve predicates through the alias table, and insert facts. The model cannot set confidence, decide overwrite semantics, or bypass sensitive confirmation. The contract is stable in Rust, so swapping models changes quality, not correctness.

The overwrite/coexistence matrix from `VISION/02-Knowledge-Graph/Learning-Modes.md` is unchanged and enforced in Rust. The shipped policy pins Casual confidence at exactly `0.30` (`mimir-knowledge/src/confidence.rs`), superseding the `0.2–0.4` design range in the VISION doc:

| New ↓ / Existing → | Explicit (1.0) | Casual (0.30) | Inferred |
|--------------------|----------------|---------------|----------|
| **Explicit (1.0)** | Overwrite      | Overwrite     | Overwrite |
| **Casual (0.30)**   | Coexist        | Coexist       | Coexist  |

## What changed in code

- `mimir-server/src/routes/chat.rs`: removed `submit_librarian_goal` and its two call sites (blocking and streaming). The `remember` tool was already registered in the tool registry, so inline learning needed no new wiring.
- `mimir-knowledge/src/tools/remember.rs`: enriched the tool description with the classification semantics and a canonical-predicate nudge, preserving extraction quality without a second LLM call.

## What was kept

The `LibrarianAgent`, `LibrarianGoal` / `LibrarianContext`, and `KnowledgeGraph::extract_facts_with_context` are retained as a library API for future on-demand and bulk-extraction use cases (e.g. a specialist research agent that invokes the Librarian explicitly). They are simply no longer auto-invoked from the chat route.

## Testing

- `test_chitchat_does_not_trigger_background_learning` — a chitchat turn where the LLM does not call `remember` records exactly one LLM call (the main chat completion) and no background extraction call. This is the regression guard for the retired unconditional Librarian.
- `test_chat_extracts_facts_after_response` — when the LLM calls `remember` inline, the fact is persisted with Rust-enforced policy.
- `mimir-knowledge/tests/librarian_agent.rs` — the Librarian still works when invoked explicitly (library API intact).

## Future

A `request_deep_research` orchestration tool (spawning the Reasoning Engine, Phase 4) will follow the same pattern: the LLM decides to invoke it; Rust owns the agent-spawn semantics. The contract stays stable across model changes.
