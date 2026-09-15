# Provenance in Retrieval

Mimir records where every learned fact came from. Retrieval now passes that provenance through the structured context the assistant sees, instead of hiding it inside the knowledge graph.

## What This Means For You

When Mimir answers from an email, calendar event, photo, or import, the retrieval result can show which connector instance produced the fact, the raw item reference, how it was extracted, and when it was extracted. This makes citations clearer and prepares Mimir to retrieve the original item behind a fact when the stored fact is too sparse.

## How It Works

`kg_query` returns a `sources` array for every fact. A connector-backed source includes the connector instance id, connector type (for example `email`), a raw item reference such as `17:42`, and the extraction method. User edits still include their source type and timestamp; connector-only fields are null. The deterministic retrieval agent carries those source records into `retrieve_context` output and merges source records when it encounters the same fact through multiple retrieval tools.

## Best Practices

- Treat the raw reference as an address, not as prose: it identifies the connector item for later follow-up.
- Check `source_type` before assuming an item is actionable or user-editable.
- Prefer facts with complete connector provenance when you need to trace or refresh a detail.
- Do not expect retrieval to fetch the external item yet. That bounded source-fetch strategy is handled separately by issue #495.

## Related

- `docs/retrieval-agent.md`
- `docs/kg-tools.md`
- Issue #491 — query-time source fetch
