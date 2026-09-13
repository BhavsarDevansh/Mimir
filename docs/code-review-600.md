# Code Review — Issue #600

> **Status:** Reviewed after implementation and documentation.
>
> **Result:** Zero findings remain.

| Dimension | Finding | Severity | Action |
|---|---|---|---|
| Code quality | The extraction guide retained the synthetic `Root` node, which does not represent a fact classification. | Medium | Added a prompt-local structural-root boundary and omitted only category `0`. |
| Correctness | Structural-root exclusion needed to be pinned without removing the root from taxonomy state. | Medium | Added prompt and catalogue tests proving Root remains available to tree APIs while absent from extraction. |
| Prompt safety | Non-Root parentless categories must remain selectable so user-added top-level domains are not lost. | None | Verified with a new assignable root-category prompt test. |
| Performance | Root exclusion uses the existing one-shot category list and adds O(1) checks per category. | None | No further action. |
| Security | Category names remain rendered through the existing prompt-safe normaliser; no raw boundary was added. | None | No further action. |
| DRY compliance | The structural boundary is represented by one shared constant rather than duplicated magic values. | Low | Centralised the check in `is_structural_category`. |
| Documentation | Extraction docs still described the guide as a complete category tree. | Low | Updated technical and wiki docs to distinguish assignable categories from the synthetic Root. |
| Versioning | A workspace release increment was required for the bugfix. | Low | Bumped the workspace and feature-level version stamps to `0.166.4`. |
| TDD | The fix was regression-prone and needed a failing test first. | Low | Added failing prompt tests before implementing the exclusion. |
| Safety | No unsafe code or environment mutation was introduced. | None | No further action. |
