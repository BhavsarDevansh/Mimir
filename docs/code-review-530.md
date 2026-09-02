# Code Review — Issue #530

## Findings

| Dimension | Finding | Severity | Resolution |
|---|---|---|---|
| Code quality | The timeout test initially duplicated the production 10-second constant in test setup. | Low | Reused `DEFAULT_START_TIMEOUT` for the existing default mock guard while injecting a 100 ms budget in the timeout test. |
| Performance | The original timeout regression slept through the full 10-second production budget. | High | Injected a 100 ms test timeout and kept the normal 10-second default unchanged. |
| Security | The timeout is an internal test-only field and cannot be influenced by user input. | None | No action required. |
| DRY compliance | The initial backoff delay was coupled separately to the timeout value. | Low | Derived the initial delay from `start_timeout` with a 20 ms minimum, preserving the production 200 ms start. |
| Guideline compliance | No unsafe code or process-environment mutation was introduced. | None | No action required. |
| Public API surface | The daemon guard API remained unchanged; the new field is private and internal. | None | Documented the injectable internal timeout in the technical and user-facing guides. |
| Documentation | The benchmark tracking list still identified #530 as open. | Low | Removed #530 from the open performance issue list and recorded the injectable timeout behavior. |
| Versioning | The workspace version needed a patch bump for the test-performance fix. | Low | Bumped the workspace version from `0.153.10` to `0.153.11`. |
| Correctness | The polling loop allowed one slow probe to finish after the start budget had expired, so the documented cap could overshoot by up to the probe timeout. | High | Bounded each poll with `tokio::time::timeout`, returned immediately on probe-time expiry, and capped the following sleep to the remaining budget. |
| Testability | The timeout tests did not cover a probe that outlived the start budget. | Low | Added `test_start_timeout_caps_slow_probe` with a fast initial probe followed by a slow polling probe. |
| Correctness | The retry delay used remaining time measured before a failed probe, allowing a near-deadline probe to schedule a sleep past the start budget. | High | Recalculated the remaining budget after the probe and skipped the sleep once the deadline had expired. |
| Testability | The timeout tests did not cover a delayed probe that returned `false` before the deadline. | Low | Added `test_start_timeout_recalculates_deadline_after_slow_probe` with a 90 ms probe and a 100 ms budget. |
| Documentation | The daemon-guard test table and binary test count omitted the probe, child-environment, and new timeout tests. | Low | Expanded the test table and updated the count to 13. |

## Verification

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `scripts/perf-baseline.sh`
