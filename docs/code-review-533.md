# Code Review — Event-Driven Core Test Waits

This review covers every file touched by the issue #533 change set after documentation was updated. All findings were actioned, and the review now returns zero open findings.

| Dimension | Finding | Severity | Action |
| --- | --- | --- | --- |
| Correctness | The first context timestamp-ordering update ran before the second session message, so the production update could overwrite the explicit ordering timestamp. | High | Move both explicit `updated_at` writes after both messages and re-run the test. |
| Test reliability | Scheduler gate-check notifications could be consumed by the wrong loop iteration, and the old cooldown test added a 20 ms race guard. | Medium | Replace per-check notify signals with a monotonic watch sequence and await exact gate-check generations. |
| Test reliability | Hook timing tests used elapsed-time guesses to infer loop readiness. | Medium | Await a test-only hook gate-check sequence and retain Tokio paused time for windows. |
| Test reliability | Agent-runtime and worker-pool tests slept before observing background-task state. | Medium | Add test-only task-exit and job-start signals, bounded to five seconds. |
| API surface | Removing the derived `Default` implementation from `AgentRuntime` changed the type’s usable API surface. | Low | Add an explicit `Default` implementation that delegates to `AgentRuntime::new`. |
| Documentation | The original issue body omitted newly identified sleeps and did not describe the final signal-based design. | Low | Update issue #533 and the technical/user documentation to the current test scope and mechanics. |
| Performance | The issue required a test-suite performance delta, but cargo-nextest was unavailable locally and the fallback includes compile time. | Low | Record the local `mimir-core` fallback timing in the PR and avoid claiming an exact nextest aggregate delta. |

## Validation

`cargo fmt -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace --exclude mimir-knowledge` pass. The full workspace run exposed the pre-existing Obsidian failures already tracked in issue #604.
