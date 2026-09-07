# Retrieval Agent

## What Is It?

When Mimir needs facts or historical context to answer your question, it launches a focused research stage through the `retrieve_context` tool. This stage searches your knowledge graph and past conversations, then gives the answer model structured evidence.

It can look up facts about people and places you mention, traverse relationships, and find relevant past snippets.

## How It Works

For a request such as:

> "I have Mary, Bob, and Tom coming over for dinner. What should I make?"

the main model may launch separate retrieval tasks for Mary, Bob, and Tom. Each task is researched in one deterministic pass:

1. Mimir searches entities once for each distinct alphanumeric token in the task, up to a fixed cap that keeps long or adversarial tasks from flooding the database with searches.
2. It searches conversation history once with the salient terms of the task (common filler words dropped and the terms OR-joined), so a past message only needs to mention one of them to surface.
3. For each distinct entity found, it queries facts once and traverses related relationships once, with a cap on how many candidates are expanded. Fact queries paginate in bounded pages, so an entity with many facts is not silently cut off.
4. Duplicate entities, facts, relations, and snippets are merged before the main model sees the context.

The inner retrieval process does not ask an LLM to choose tools or decide when to stop. This removes repeated identical calls, runaway loops, and duplicate database work while keeping the main model responsible only for choosing whether to request context and synthesising the answer.

## What You See

In streaming chat, research steps are visible as normal tool-call events:

```text
event: tool_call_start
{"name": "kg_search", "display_name": "KG Search"}

event: tool_call
{"name": "kg_search", "display_name": "KG Search", "result": "..."}
```

Blocking requests use the same retrieval logic but do not stream nested progress events.

## When It Helps

The main model should use `retrieve_context` when the answer may depend on personal facts, entity relationships, or conversation history rather than only the condensed memory block.

## Limitations

- Candidate discovery is token-based and searches the local knowledge graph; it does not use an LLM to invent entity names.
- Relationship traversal remains bounded by the existing `kg_related` limits.
- Planning is bounded by fixed caps on task length, search tokens, salient conversation terms, expanded candidates, and concurrent tool calls, and `kg_query` fetches at most a fixed number of fact pages per candidate (larger fact sets are truncated and flagged as `truncated` rather than silently reduced).
- External APIs and side-effecting tools are not used by this stage.
- A transient tool failure is omitted from that step's evidence; the retriever does not retry it.

The broader recall engine work in issues #569–#571 will expand this boundary with richer query contracts, ranking, provenance, and token budgeting.
