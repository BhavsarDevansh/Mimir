# Code Review — Issue #534

## Scope

Reviewed `mimir-core/src/hooks/mod.rs`, `mimir-core/src/hooks/tests.rs`, `mimir-server/tests/common/mod.rs`, the changed `mimir-server` integration suites, and the issue-specific documentation.

## Findings

| Dimension | Finding | Severity | Resolution |
|---|---|---|---|
| Test correctness | The initial chat-hook idle wait used engine-wide `running_count`, which could be blocked by an unrelated hook and reintroduce flakiness. | High | Added `HookEngine::is_running(hook_id)` and used the hook-specific readiness state. |
| Code quality | The server tests repeated deadline and polling logic for different observable states. | Medium | Added one shared bounded `poll_until` helper and reused it across all affected tests. |
| Documentation | The issue body still described removed startup/stop sleeps. | Low | Refreshed issue #534 to match the current server test code. |
| Public API surface | The hook-specific readiness method is part of the public hook observability surface. | Low | Documented `is_running(hook_id)` alongside the existing observability methods. |

## Verification

All findings were actioned. `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, the targeted hook and server suites, and the new helper tests pass. The workspace test run reached the two known main failures tracked by issues #597 and #604 in `mimir-knowledge`; these are unrelated to this change.
