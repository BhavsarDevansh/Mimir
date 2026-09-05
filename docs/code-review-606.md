# Code Review — PR #606

## Scope

Reviewed `mimir-core/src/hooks/mod.rs`, `mimir-core/src/hooks/tests.rs`, and `mimir-server/tests/common/mod.rs`, including the shared polling helper and the public hook observability surface.

## Findings

| Dimension | Finding | Severity | Resolution |
|---|---|---|---|
| Test correctness | `TestHandler::release` used `notify_waiters`, but `force_run` can observe running state before the handler registers its waiter, allowing the release signal to be lost. | Medium | Changed release to `Notify::notify_one`, whose retained permit is safe when the handler has not yet registered. |
| Test correctness | Separate pending-depth and running checks could observe an idle gap while a failed hook moved from running back to pending. | High | Added `HookEngine::is_settled_for` and serialised the dispatch retry transition behind a dedicated synchronization boundary. |
| DRY compliance | `wait_for_chat_hook_idle` duplicated bounded polling and timeout logic while trying to bound mutex-backed state reads. | Medium | Extended shared `poll_until` to wrap predicate evaluation in the remaining timeout and cap polling delay, then reused it for settled hook state. |
| Public API surface | The new hook observability method was not documented. | Low | Documented `is_settled_for` in the hooks API surface. |
| Versioning | The new public hook method required a minor version bump rather than retaining the prior patch version. | Low | Bumped the workspace version to `0.160.0`. |

## Verification

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, targeted hook tests, and targeted server integration tests pass. The full workspace test run is blocked by the pre-existing Obsidian import failures tracked in issue #607; no PR #606 files are affected by those failures.
