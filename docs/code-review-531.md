# Code Review — Issue #531

## Findings

| Dimension | Finding | Severity | Resolution |
|---|---|---|---|
| Correctness | The LLM retry test assumed the default four-attempt budget while it used the proposed fast schedule. | High | Updated the persistent-failure test to assert exactly two attempts and preserve the upstream 503 error. |
| Performance | Retry tests slept through 400/800/1600 ms LLM backoff and two one-second connector `Retry-After` waits. | High | Added an injectable `RetryConfig` to `LlmClient`, propagated it to pooled workers, and bounded connector `Retry-After` handling to the injected strategy cap. |
| Correctness | The backoff helper mixed duration and integer-millisecond conversions, adding avoidable overflow handling and truncation risk. | Medium | Changed backoff calculation to saturating `Duration` arithmetic and sleep directly with the resulting duration. |
| Public API surface | A new retry configuration and a changed worker-pool constructor needed explicit contracts. | Medium | Documented `RetryConfig`, the total-attempt semantics, pool propagation, direct constructor signature, and production defaults in the LLM client and worker-pool guides. |
| Testability | The connector HTTP test could not exercise `Retry-After` without waiting two seconds because its strategy had no maximum. | Medium | Replaced the fixed strategy with a 1–10 ms exponential strategy; the test now verifies real `Retry-After` clamping without real waiting. |
| DRY compliance | The integration helper would otherwise have repeated fast retry configuration in each retry test. | Low | Kept a single `test_client` helper that applies the injected fast schedule for the HTTP integration suite. |
| Type consistency | The attempt count is naturally larger in the error type but small enough in configuration for `u8`. | Low | Stored `max_attempts` as `u8` and converted it explicitly to `u32` only at the retry boundary. |
| Guideline compliance | The change could have introduced unsafe code or process-environment mutation. | None | No action required; the change contains neither. |
| Documentation | The benchmark tracking list still described #531 as open. | Low | Removed #531 from the open list and documented the injectable retry behavior in technical and wiki documentation. |
| Versioning | The workspace version needed a minor bump for the backwards-compatible public retry configuration. | Low | Bumped the workspace version from `0.153.11` to `0.154.0`; active release notes remain on GitHub Releases per project policy. |

## Verification

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- Targeted retry tests completed in 0.08–0.12 s.
