# Retrieval Agent

## Overview

The **Retrieval Agent** is the deterministic research stage behind Mimir's `retrieve_context` tool. It gathers structured context from the knowledge graph and conversation history before the main LLM composes an answer. It implements the original pre-response retrieval design in issue #128, with the loop-control bug from issue #492 fixed by removing the LLM-controlled inner loop. This provenance slice implements #494.

## Architecture

```text
Main LLM
    ↓  calls retrieve_context tool
RetrieveContextTool
    ↓  builds a fixed Rust plan
RetrievalAgent
    ↓  executes each distinct step once
    ├─ kg_search (one query per task token)
    ├─ search_conversation_history (one salient-term OR query)
    ├─ kg_query (once per distinct candidate entity)
    └─ kg_related (once per distinct candidate entity)
    ↓  accumulates structured results
RetrievedContext (JSON)
    ↓  passed back to
Main LLM via ToolOutput
```

## Key Design Decisions

- **Rust owns control flow**: Retrieval builds a fixed plan, executes it, and returns. The retrieval LLM cannot choose tools, repeat calls, or decide when the work is complete.
- **No caching or call budgets**: Correctness comes from deterministic planning rather than duplicate-call caching, per-tool quotas, or empty-result cutoffs.
- **Candidate discovery**: Each alphanumeric task token is used for one knowledge-graph entity search. This preserves multi-entity tasks such as "Mary Bob" without relying on a single phrase query. Distinct token searches are capped per task, and the task text consulted during planning is capped so tokenisation and dedupe scans stay bounded regardless of task length.
- **Follow-up completeness**: Every distinct candidate found by search is queried once for full facts and once for bounded relationship traversal. Repeated names are normalised during plan construction, and the candidate set is capped so a wide search cannot fan out without bound. `kg_query` fetches a candidate's facts page by page (bounded pages of 50) so nothing is silently truncated; when facts remain beyond the final page the merged output carries `"truncated": true` and the truncation is logged.
- **Conversation evidence**: Conversation history is searched once with a relaxed query built from salient task terms (filler words dropped, tokens) passed with `match_any: true`, so a message containing any single salient term surfaces. This keeps natural-language tasks such as "Find Mary's food preferences and any allergies" from failing under all-token AND matching. Tasks without any salient term fall back to the full task under the same relaxed mode.
- **Bounded fan-out**: A single retrieval task can never spawn an unbounded number of concurrent database queries. Task, token, candidate, and salient-token caps bound the plan, and steps execute in fixed-size chunks so at most a constant number of tool futures are in flight.
- **Possessive handling**: Possessive suffixes (`'s`, `’s`) are stripped case-insensitively before tokenisation, so `JAMES'S` and `James’s` never plan a junk `S` search.
- **Structured output**: `RetrievedContext` contains entities, facts, relations, conversation snippets, `finish_reason`, and `steps_executed`.
- **Provenance preservation**: `kg_query` sources carry connector instance id, connector type, raw reference, extraction method, source type, and extracted timestamp. When the same structural fact is seen first through `kg_search` and later through `kg_query`, the deterministic merge updates the existing fact with the newly discovered sources instead of emitting a duplicate.
- **Temporal context**: Facts retain RFC 3339 UTC `valid_from` and `valid_until` bounds across `kg_query` and `kg_search`.
- **Source identity**: Facts retain complete source provenance, including the connector instance and raw reference needed by the later query-time source-fetch strategy (#495). This slice does not fetch external data; it only makes the pointer available deterministically.
- **Error resilience**: A failed retrieval step is logged and omitted, while other steps continue. The retriever does not retry automatically or turn transient errors into an empty-result signal.
- **Progress events**: On streaming requests, each step emits `ToolProgress::Started` and `ToolProgress::Finished` through the existing `ToolContext` factory path (issue #487). Blocking paths pass no channel and run silently.

## Data Types

`RetrievedContext` has this shape:

```rust
pub struct RetrievedContext {
    pub entities: Vec<RetrievedEntity>,
    pub relations: Vec<RetrievedRelation>,
    pub conversation_snippets: Vec<ConversationSnippet>,
    pub finish_reason: Option<String>,
    pub steps_executed: u16,
}
```

`finish_reason` is now deterministic (`completed`) rather than model-supplied. `steps_executed` counts the fixed search and follow-up steps that were attempted.

## Files

- `mimir-knowledge/src/retrieval/agent.rs` — deterministic retrieval executor
- `mimir-knowledge/src/retrieval/types.rs` — `RetrievedContext`, `RetrievedFact`, and `RetrievedSource`; `RetrievedFact::merge_sources` deduplicates equivalent source records while retaining their order
- `mimir-knowledge/src/tools/retrieve_context.rs` — `RetrieveContextTool` exposed to the main LLM
- `mimir-knowledge/tests/retrieval_tests.rs` — deterministic retrieval tests

## Integration

`retrieve_context` is registered in `mimir-server/src/state/builder.rs` alongside other knowledge-graph tools. The main LLM can call it through the normal tool loop, but once it does, the inner retrieval process is fully deterministic. The future unified query contract and deterministic planner from #569 and #570 will build on this boundary.
