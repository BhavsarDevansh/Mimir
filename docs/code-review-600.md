# Code Review — Issue #600

> **Status:** Reviewed after implementation and documentation.
>
> **Result:** Zero findings remain.

| Dimension | Finding | Severity | Action |
|---|---|---|---|
| Code quality | The extraction guide retained the synthetic `Root` node, which does not represent a fact classification. | Medium | Added a prompt-local structural-root boundary and omitted only category `0`; categories parented by the Root are promoted to top-level assignable entries. |
| Correctness | Structural-root exclusion needed to be pinned without removing the root from taxonomy state. | Medium | Added prompt and catalogue tests proving Root remains available to tree APIs while absent from extraction. |
| Prompt safety | Non-Root parentless categories and categories parented by Root must remain selectable so user-added top-level domains are not lost. | Medium | Verified with prompt tests for both top-level forms. |
| Correctness | Omitting Root also orphaned categories parented by it because the guide starts only from `parent_id IS NULL`. | Medium | Promoted Root-child categories to top-level guide entries and pinned the behavior with a test. |
| Performance | Root exclusion uses the existing one-shot category list and adds O(1) checks per category. | None | No further action. |
| Security | Category names remain rendered through the existing prompt-safe normaliser; no raw boundary was added. | None | No further action. |
| DRY compliance | Prompt tests previously duplicated the root-id magic value. | Low | Reused `STRUCTURAL_CATEGORY_ID` and `is_structural_category` in tests and centralised the runtime check. |
| Documentation | `is_structural_category` lacked a doc comment and extraction docs did not explain the Root-child promotion. | Low | Added the doc comment and updated technical and wiki docs to distinguish assignable categories from the synthetic Root. |
| Versioning | A workspace release increment was required for the bugfix. | Low | Bumped the workspace and feature-level version stamps to `0.166.4`. |
| TDD | The root-child edge case was regression-prone and needed a failing test first. | Low | Added a failing root-child prompt test before implementing the promotion. |
| Safety | No unsafe code or environment mutation was introduced. | None | No further action. |
