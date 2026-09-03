# Code Review — Issue #471

## Findings

| Dimension | Finding | Severity | Resolution |
|---|---|---|---|
| Correctness | `kg_search` top facts dropped both temporal bounds, unlike `kg_query`. | High | Added `valid_from` / `valid_until` to the search fact query, output type, and retrieval accumulation. |
| Correctness | Upcoming one-time lines could omit a fact's known end bound. | High | Carried `valid_until` into the upcoming query and passed it through the shared renderer. |
| Correctness | Recurring upcoming lines used only the occurrence time and no fact interval end. | Medium | Kept the occurrence time as the line's start bound and preserved the fact's `valid_until`. |
| Performance | The memory character estimate ignored the newly appended ISO bounds. | Medium | Extended the estimate and truncation allowance to include the shared bounds suffix. |
| DRY compliance | The retrieval parser repeated the fact-to-`RetrievedFact` mapping for `kg_query` and `kg_search`. | Medium | Added one `parse_retrieved_fact` helper used by both accumulation paths. |
| DRY compliance | ISO bounds formatting and length estimation could drift. | Medium | Added one renderer-owned bounds formatter and derived the budget estimate from it. |
| Type consistency | `RankedFact` lacked the temporal fields needed by the renderer. | Medium | Added typed `Option<DateTime<Utc>>` bounds and propagated them from the SQL row. |
| Public API surface | `RankedFact` and search fact output gained required fields. | Medium | Documented the new fields and updated tests, fixtures, and bench constructors. |
| Documentation | The memory, KG-tool, and retrieval guides did not state the ISO UTC bounds contract. | Low | Updated the technical and wiki documentation with examples and the shared temporal contract. |
| Test coverage | Existing tests only pinned prose with no temporal inputs. | High | Added unit, integration, and retrieval tests for `valid_from` only, both bounds, neither, search output, upcoming output, and budget propagation. |
| Guideline compliance | The change could have introduced unsafe code or global environment mutation. | None | No action required; the change contains neither. |
| Versioning | The workspace version needed a patch bump for the backwards-compatible rendering and tool-output addition. | Low | Bumped the workspace version from `0.157.0` to `0.157.1`. |
| Test hygiene | Two unrelated Obsidian import tests failed during the workspace run. | Low | Filed #591 for the unrelated em-dash and dry-run count regressions. |

## Verification

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Targeted memory, `kg_search`, retrieval, ranking, and upcoming-event tests pass.
- `cargo test --workspace --no-fail-fast --all-features` passes for every affected target; the unrelated `mimir-knowledge --test obsidian_test` target fails and is tracked by #591.
