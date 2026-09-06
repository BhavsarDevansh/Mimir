# Code Review — Issue #536

## Review Findings

| Dimension | Finding | Severity | Action |
|---|---|---|---|
| Testing correctness | A wall-clock timeout in the regression test could be sensitive to heavily loaded CI hosts. | Low | Paused Tokio time after database setup and used a fixed 100 ms virtual timeout, removing wall-clock scheduling sensitivity without breaking SQLx pool initialisation. |
| Documentation consistency | The benchmark table did not record the post-fix measurement, leaving only the stale 5.08 s baseline. | Low | Added the local post-fix benchmark result and clarified that it now measures ordinary teardown. |
| Concurrency correctness | Creating the exit-waiter future after the shutdown signal could miss a rapid initial-loop exit and reintroduce the full timeout. | Medium | Moved `Notified` registration ahead of the shutdown send while retaining the unstarted-loop early return; the future is now registered before any matching `notify_waiters`. |

## Final Review Status

All findings were actioned. The review returns zero open findings. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the targeted hook tests pass. The full workspace run has only the pre-existing Obsidian failures tracked by #608. The state-build benchmark dropped from the 5.08 s fixed timeout baseline to 114 ms for the local post-fix run.
