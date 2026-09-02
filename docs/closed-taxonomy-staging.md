# Closed Taxonomy and Fact Staging

## Implementation

Mimir keeps the relationship vocabulary in SQLite instead of relying on a static Rust enum. Migration 060 adds `parent_id`, `depth`, `node_kind`, `emit_eligible`, definition, renderer, deduplication, temporal, and sensitivity fields to `relationship_types`; adds `relationship_type_category_rules`; and creates `unrecognized_facts`. A root is query-only, an intermediate node is query-only, and a leaf is the only node kind eligible for emission as a fact predicate. The positive-preference leaf is `prefers`; legacy positive forms such as `likes`, `loves`, `favourite_food`, and `favourite_colour` resolve through controlled aliases rather than competing predicates.

`KnowledgeGraph::resolve_emit_eligible_relationship_type` is the strict ingestion resolver: it resolves controlled names and aliases through the database, rejects query-only nodes, and never creates a row. Every fact-insertion path now uses this resolver, including direct fact writes and batch ingestion, so unknown predicates cannot slip into the graph by bypassing normalization. `KnowledgeGraph::default_category_id_for_relationship_type` supplies the deterministic category fallback at `normalize_and_insert`, direct insert, and batch insert boundaries. Producer-supplied categories remain refinements; when none are valid or supplied, the rule table supplies one so every fact still belongs to the catalogue tree.

The email prose-extraction layer now stages invalid LLM-emitted facts in `unrecognized_facts` instead of silently dropping them; a failed staging write retries the email rather than proceeding without the record. Staging identity uses nullable-aware source keys plus the raw predicate and serialized payload, so retries and repeated chat output with identical content deduplicate instead of flooding review. Both email and conversational extraction generate their `relationship_type` tool-schema enum from `list_emit_eligible_relationship_type_names`, so the model sees the same closed vocabulary the Rust resolver checks. The hook counts only newly staged records in `facts_staged` alongside `facts_accepted` (inserted or pending confirmation) and `facts_dropped`, so `mimir connector list` and `status` expose vocabulary regressions instead of hiding them behind an item count.

Governance is exposed through `GET /kb/staged`, `POST /kb/staged/{id}/map`, and `POST /kb/staged/{id}/reject`. The CLI mirrors this with `mimir kb staged list`, `mimir kb staged map <id> --relationship-type-id <leaf-id>`, and `mimir kb staged reject <id>`. Mapping validates that the target is an emit-eligible leaf, so a staged row can never be mapped back to a query-only root or intermediate node.

## Rationale

A static compile-time enum is safe but hard to grow. A fully open predicate string recreates the alias and duplicate-fact failures that motivated issue #468. The controlled DB taxonomy keeps the LLM contract closed while allowing human-approved vocabulary and frame evolution without a code change for every domain-specific leaf.

Typed entity frames are the intended home for domain richness. For example, a hotel booking should be a `Booking` entity with structured check-in/check-out attributes rather than two global predicates. This keeps the relation tree small and makes composite facts queryable without inventing a new verb for every provider or document type.

## System connections

- `normalize_and_insert` owns category fallback and strict relationship resolution.
- Email LLM extraction owns durable staging of unrecognized output.
- `GET /kb/staged` and `mimir kb staged` provide human governance for staging review.
- Connector status surfaces accepted, dropped, and staged counts.
- Extraction schemas and prompts read the same DB taxonomy rather than duplicating leaf lists in prompts. The remaining static `CANONICAL_PREDICATES` list is a seed/registration pin for tests and deterministic connectors, not the runtime LLM contract.
