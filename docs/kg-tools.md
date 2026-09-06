# Knowledge Graph LLM Tools

## Overview

Three LLM-callable tools expose the knowledge graph to the agent:

- **`kg_query`** — retrieve facts for a named entity with optional predicate, confidence, and pagination filters.
- **`kg_related`** — breadth-first traversal from a named entity to discover related nodes.
- **`kg_search`** — FTS5 full-text search over entities, surfacing top facts per match.

All tools implement the `mimir_core::Tool` trait and are registered in the server's `ToolRegistry` on startup.

## Tool Schemas

### `kg_query`

```json
{
  "type": "object",
  "properties": {
    "entity_name": { "type": "string" },
    "predicate":    { "type": "string" },
    "min_confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
    "offset": { "type": "integer", "minimum": 0 },
    "limit":  { "type": "integer", "minimum": 1, "maximum": 50 },
    "include_subtree": { "type": "boolean", "default": false }
  },
  "required": ["entity_name"]
}
```

`kg_search` returns each fact's `valid_from` and `valid_until` as RFC 3339 UTC values when present, matching the temporal context supplied by `kg_query`.

### `kg_related`

```json
{
  "type": "object",
  "properties": {
    "entity_name":    { "type": "string" },
    "max_depth":      { "type": "integer", "minimum": 1, "maximum": 5 },
    "max_nodes":      { "type": "integer", "minimum": 1, "maximum": 200 },
    "predicate_filter": { "type": "array", "items": { "type": "string" } }
  },
  "required": ["entity_name"]
}
```

### `kg_search`

```json
{
  "type": "object",
  "properties": {
    "query":        { "type": "string" },
    "entity_type":  { "type": "string" },
    "limit":        { "type": "integer", "minimum": 1, "maximum": 20 }
  },
  "required": ["query"]
}
```

## Query Implementation Details

### `kg_query`

1. Resolve `entity_name` via `queries::entity::get_by_name` (exact → alias → FTS5 fuzzy).
2. If a `predicate` is provided, resolve it via the read-only `KnowledgeGraph::get_predicate_id` method. Missing predicates cause empty results (no predicate is inserted).
3. Query `queries::fact::get_facts_by_subject_filtered` with:
   - `pending_confirmation = 0`
   - `fact_status_id NOT IN (5, 6)` (excludes Superseded, Forgotten)
   - optional `predicate_id` and `confidence >= ?`
   - SQL-level `ORDER BY confidence DESC, valid_from DESC LIMIT ? OFFSET ?`
4. Batch-fetch sources per result page and attach to output.

### `kg_related`

Implements Rust-level BFS with the following characteristics:

- **Cycle detection:** `visited: HashSet<u32>` ensures each entity is expanded at most once.
- **Per-level batched queries:** a single SQL query with `subject_id IN (...)` resolves all edges for the current frontier.
- **Bounded reads:** each level applies a SQL `LIMIT` of `remaining_budget * 2` so SQLite does not read unbounded rows from high-degree entities.
- **Predicate filtering:** optional `predicate_filter` restricts edges to a whitelist of predicates, resolved via the read-only `KnowledgeGraph::get_predicate_id` method. Missing predicates are skipped (not inserted). (Previous documentation incorrectly referenced `ensure_predicate`.)
- **Batch name resolution:** `queries::entity::get_entity_names` resolves all subject and object names in one query per level.

Stop conditions: `depth >= max_depth`, `visited.len() >= max_nodes`, or empty frontier.

### `kg_search`

1. Escape the raw query via `queries::entity::escape_fts5` (wraps in double quotes, doubles internal quotes, replaces `*` with spaces).
2. FTS5 `MATCH` against `entity_fts`, joined to `entities` and `entity_types`.
3. Optional `entity_type_id` filter.
4. SQL `ORDER BY rank LIMIT ?`.
5. Batch-fetch facts for all matched entities in a single query, then group in Rust to top 5 per entity. The fact batch carries `valid_from` / `valid_until` so search results match `kg_query`'s temporal contract.

## Security

- **Input length caps:** `entity_name` ≤ 200 chars, `query` ≤ 500 chars, `predicate_filter` ≤ 10 items (each ≤ 200 chars). Rejected with `ToolError::InvalidArguments`.
- **FTS5 injection defense:** `escape_fts5` neutralises boolean operators, asterisks, and nested quotes.
- **Pending facts excluded:** every fact query filters `pending_confirmation = 0` at the SQL level.
- **Soft-deleted facts excluded:** `fact_status_id NOT IN (5, 6)` excludes Superseded and Forgotten. Disputed facts (`status_id = 3`) are intentionally retained so the LLM can reason about contradictions.
- **Safe error exposure:** entity-not-found returns a `ToolOutput` with `error` field, not an internal Rust error.

## Performance

- **Composite indexes:** `idx_facts_tool_query ON facts(subject_id, pending_confirmation, fact_status_id, confidence DESC)` covers the hot path for all three tools, while `idx_facts_subject_relationship ON facts(subject_id, relationship_type_id)` backs subject-plus-predicate scans used by insertion, traversal, and deduplication.
- **SQL pagination:** `kg_query` never fetches all rows into Rust memory.
- **FTS5-level limiting:** `kg_search` applies `LIMIT` inside the FTS5 subquery.
- **No N+1:** `kg_search` batch-fetches facts and object names; `kg_related` batch-resolves entity names per BFS level.

## Error Handling Strategy

| Condition                  | Error type                          | LLM-visible? |
|----------------------------|-------------------------------------|--------------|
| Invalid JSON args          | `ToolError::InvalidArguments`       | Yes (via error field) |
| Input exceeds length cap   | `ToolError::InvalidArguments`       | Yes |
| Entity not found           | `ToolOutput.error = Some(...)`      | Yes |
| Database error             | `ToolError::ExecutionFailed`        | Generic message |
| JSON serialization error   | `ToolError::ExecutionFailed`        | Generic message |

## Files

- `mimir-knowledge/src/db/migrations/028_add_performance_indexes.sql`
- `mimir-knowledge/src/queries/search.rs`
- `mimir-knowledge/src/queries/traverse.rs`
- `mimir-knowledge/src/queries/entity/` (additions)
- `mimir-knowledge/src/queries/fact/` (additions)
- `mimir-knowledge/src/tools/kg_query.rs`
- `mimir-knowledge/src/tools/kg_related.rs`
- `mimir-knowledge/src/tools/kg_search.rs`
- `mimir-knowledge/src/tools/mod.rs`
- `mimir-knowledge/src/lib.rs` (exports)
- `mimir-server/src/state/` (integration)

## Batch Insertion

`KnowledgeGraph::insert_facts_batch(Vec<NewFact>)` inserts multiple facts atomically in a single SQLite transaction. It resolves relationship types, validates categories, resolves each connector instance to its registered type and caches reliability scores per distinct type (so batch-inserted connector facts with a `connector_instance_id` get their adjusted table score), writes rows via `queries::fact::insert_fact_in_tx`, and assigns categories — all inside one `BEGIN … COMMIT` block. Centrality bumps run after the transaction commits. Rule-engine passes are skipped; callers should trigger them separately if needed.

## Targeted Predicate Lookup

`KnowledgeGraph::relationship_type_id(&str)` performs a cached, non-mutating lookup of a relationship type by name, returning `None` if it does not exist; there is no public relationship-type creation helper, so missing rows require a migration or governance-approved vocabulary change.

`KnowledgeGraph::get_facts_by_subject_and_predicate(subject_id, relationship_type_id)` returns only facts matching a specific subject–predicate pair, avoiding full-table scans.

## Relationship-Type Subtree Expansion

When `kg_query` is called with `include_subtree: true` and a `predicate`, the predicate is resolved to a canonical `relationship_type_id` (alias-aware) and facts are fetched via `queries::fact::get_facts_by_relationship_subtree` instead of `get_facts_by_subject_filtered`. The query walks the `relationship_type_hierarchy` DAG with a single recursive CTE that seeds with the root type itself, so facts of the predicate **and all descendant types** are returned:

```sql
WITH RECURSIVE subtree(id) AS (
    SELECT ?
    UNION
    SELECT h.child_id FROM relationship_type_hierarchy h
    JOIN subtree s ON h.parent_id = s.id
)
SELECT f.*, e.name AS object_name
FROM facts f
JOIN subtree s ON f.relationship_type_id = s.id
LEFT JOIN entities e ON e.id = f.object_id
WHERE f.subject_id = ?
  AND f.pending_confirmation = 0
  AND f.fact_status_id NOT IN (5, 6)
  AND f.confidence >= ?
ORDER BY f.confidence DESC, f.valid_from DESC, f.id DESC
LIMIT ?;
```

The `UNION` (not `UNION ALL`) deduplicates relationship-type ids, so a type reachable via multiple hierarchy paths contributes each fact only once. Filters and ordering match `get_facts_by_subject_filtered` (non-pending, status `NOT IN (5, 6)`, confidence floor). A matching `count_facts_by_relationship_subtree` produces the `total` field. `include_subtree` without a `predicate` is rejected with `ToolError::InvalidArguments`; the subtree path has no `offset` (results are bounded by `limit` only); a caller-supplied `offset` is ignored and the response `offset` is always `0`, including the empty-result early-return when the predicate does not exist. The `KnowledgeGraph::get_facts_by_relationship_subtree(entity_id, root_type_id, limit)` wrapper is a convenience with `min_confidence = 0.0`.

Migration `051` (issue #403) seeds four query-only abstract parents so subtree expansion works out of the box: `employment` → `works_at`/`works_as`/`job_title`, `education` → `studied`/`studied_at`/`completed_degree`/`educational_status`, `residence` → `resides_in`, and `containment` → `located_in`. These parents are not in the `CANONICAL_PREDICATES` allow-list, so the conversational extraction path rejects them as fact predicates; `kg_query` resolves them through the alias table for subtree queries only.

## retrieve_context

### Tool Schema

```json
{
  "type": "object",
  "properties": {
    "task": {
      "type": "string",
      "description": "Specific research task. Describe the entity(ies) and what information you need."
    }
  },
  "required": ["task"],
  "additionalProperties": false
}
```

### Description

Launches a dedicated **RetrievalAgent** — an ephemeral LLM session with only retrieval tools — to investigate the knowledge graph and conversation history. The agent runs autonomously for up to 25 tool-call rounds, querying entities, traversing relationships, and searching past conversations. When satisfied, it calls `finish_retrieval` and returns a structured `RetrievedContext`.

`retrieve_context` is registered in the `ToolRegistry` with a **factory** (issue #441): the registry stores a prototype instance for schema export, and rebuilds the tool per request with the request-resolved LLM (model/temperature overrides) from the `ToolContext` passed to `ToolRegistry::execute`. This means the tool flows through the same dispatch path as every other tool — the registry applies the permission level (Auto/Ask/Disabled) and the incognito write-tool guard uniformly, and the chat route has no special case for it.

### Output

The tool returns a `ToolOutput` whose `result` field contains a JSON-serialized `RetrievedContext`:

```json
{
  "entities": [
    {
      "name": "Mary",
      "entity_type": "Person",
      "facts": [
        {
          "predicate": "allergic_to",
          "object_literal": "shellfish",
          "confidence": 0.95,
          "status": "Active",
          "inferred": false
        }
      ]
    }
  ],
  "relations": [],
  "conversation_snippets": [
    {
      "session_id": 42,
      "role": "user",
      "snippet": "Mary said she loved Thai food",
      "created_at": "2026-05-01T12:00:00Z"
    }
  ],
  "finish_reason": "Found all relevant preferences",
  "rounds_used": 3
}
```

The `stdout` field contains a human-readable summary:

```text
Retrieved 1 facts across 1 entities, 0 relations, and 1 conversation snippets
```

### Files

- `mimir-knowledge/src/retrieval/agent.rs`
- `mimir-knowledge/src/retrieval/types.rs`
- `mimir-knowledge/src/tools/retrieve_context.rs`
- `mimir-server/src/state/builder.rs` (registration with factory)
- `mimir-core/src/tools/registry.rs` (`ToolContext`, `ToolFactory`, `execute`)
