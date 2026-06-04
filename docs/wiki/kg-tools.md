# Knowledge Graph Tools

## What They Do

Mimir exposes three LLM-callable tools that let the agent query your personal knowledge graph:

- **`kg_query`** — look up verified facts about a specific person, place, event, or thing.
- **`kg_related`** — discover how entities are connected by following relationships through the graph.
- **`kg_search`** — find entities by name or description using full-text search.

## Example Outputs

### `kg_query`

```json
{
  "entity": { "id": 1, "name": "Alice", "entity_type": "Person" },
  "facts": [
    {
      "predicate": "lives_in",
      "object_name": "London",
      "confidence": 0.95,
      "status": "Active",
      "sources": [{ "source_type": "UserEdit", "extracted_at": "2026-06-01T10:00:00Z" }]
    }
  ],
  "total": 1,
  "offset": 0,
  "limit": 20
}
```

### `kg_related`

```json
{
  "root_entity": "Alice",
  "nodes_found": 3,
  "max_depth_reached": 1,
  "edges": [
    { "depth": 0, "subject": "Alice", "predicate": "lives_in", "object": "London", "confidence": 0.95 },
    { "depth": 0, "subject": "Alice", "predicate": "works_as", "object": "Engineer", "confidence": 0.88 }
  ]
}
```

### `kg_search`

```json
{
  "query": "London",
  "results": [
    {
      "entity": { "id": 2, "name": "London", "entity_type": "Place" },
      "match_score": 0.0,
      "top_facts": [
        { "predicate": "is_in", "object_literal": "United Kingdom", "confidence": 0.99 }
      ]
    }
  ]
}
```

## Best Practices

- **Use tools for detailed lookups.** Don't rely on the injected memory summary as an exhaustive source of facts; it is a curated overview, not a database dump.
- **Prefer `kg_query` when you know the entity name.** It returns paginated, filtered facts with provenance.
- **Use `kg_related` for exploration.** It follows relationships breadth-first and respects depth and node caps so the result stays focused.
- **Use `kg_search` when the name is uncertain.** FTS5 handles fuzzy matching and aliases automatically.
- **Respect confidence scores.** Facts with low confidence may be speculative or inferred. The default `min_confidence` of 0.5 filters out noise.

## What Is Excluded

The tools intentionally **do not** return:
- Facts awaiting user confirmation (`pending_confirmation = true`)
- Superseded or forgotten facts

Disputed facts **are** returned so the agent can surface contradictions and ask for clarification.
