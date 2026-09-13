# Code Review — Issue #619

> **Status:** Reviewed after documentation updates and the documentation-contract test.
>
> **Result:** Zero findings remain.

| Dimension | Finding | Severity | Action |
|---|---|---|---|
| Documentation accuracy | `kg_query` and `kg_related` still named the nonexistent `KnowledgeGraph::get_predicate_id`. | High | Replaced stale references with the alias-aware `KnowledgeGraph::get_relationship_type_id` lookup. |
| API clarity | The convenience lookup and its missing-predicate behavior were not documented. | Medium | Documented `get_relationship_type_id` as the primary read-only lookup, `relationship_type_id` as the convenience wrapper, and `Ok(None)`/`None` behavior for missing or failing lookups. |
| User-facing behavior | It was not explicit that unknown predicate filters never create relationship types. | Medium | Added a concise wiki note for `kg_query`/`kg_related` and a wiki relationship-type bullet explaining the no-creation contract. |
| Regression protection | Documentation could drift back to the removed method name. | Low | Added a test-only documentation-contract test that pins `get_relationship_type_id` and rejects `get_predicate_id`/`ensure_predicate`. |
| Versioning | A documentation release increment was required. | Low | Bumped the workspace version to `0.166.5` and the feature-level wiki version stamp. |
| Safety | No runtime code path was changed. | None | No unsafe code or environment mutation was introduced. |
