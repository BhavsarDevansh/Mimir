# Code Review — Issue 410

## Findings

| Dimension | Finding | Severity | Action |
|---|---|---|---|
| Code quality | The existing extraction prompt literal is a large single-line string and mixes rule sections with taxonomy data. | Medium | Extracted the category tree into a dedicated DB-driven helper; filed #598 for the broader prompt-template refactor. |
| Performance | Rendering the taxonomy by querying children per node would create N+1 database calls as the tree grows. | High | Added `list_all_categories` and built the tree from one ordered query with iterative traversal. |
| Security | User-controlled category names could contain line breaks that break prompt structure. | Medium | Normalised carriage returns and line feeds while rendering category names, and filed #599 for stronger boundary validation. |
| Taxonomy correctness | The synthetic `Root` category is parentless and therefore appears in the extraction guide as though it were assignable. | Low | Filed #600 to separate structural categories from assignable categories. |
| Testing | A test asserting only the three issue examples could pass while another descendant was omitted. | Low | Added full-taxonomy coverage, indentation assertions, a dynamically inserted deep category, prompt-budget, and prompt-safety tests. |
| Documentation | Public API and extraction behaviour were not documented for full-tree rendering. | Low | Updated README, technical schema/pipeline/librarian docs, wiki user docs, and current-capability status. |

## Result

All findings in this change set are actioned. The remaining work is tracked in #598, #599, and #600 as focused follow-ups rather than scope creep in the #410 change.
