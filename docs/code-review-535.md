# Code Review — Issue #535

## Scope

Reviewed `Cargo.toml`, `mimir-test-support/src/lib.rs`, `mimir-test-support/Cargo.toml`, the three shared test-helper modules, the knowledge-graph benchmark and manifests, and the changed technical and wiki documentation.

## Findings

| Dimension | Finding | Severity | Resolution |
|---|---|---|---|
| Test correctness | The issue said all integration tests use `TestGraph`, but many schema and migration tests call `KnowledgeGraph::init` directly. | Medium | Updated issue #535 to name the shared fixtures explicitly and keep direct migration calls intentional. |
| Code quality | A synchronous `std::sync::OnceLock` could not initialise an async template without blocking a runtime worker. | High | Switched to Tokio `OnceCell` with an async initialiser and a mutex guard. |
| Error handling | Wrapping a cached template error in `KnowledgeError::Validation` lost the original error kind. | Medium | Added a typed `TestSupportError` and a mutex/once-cell flow that retries cleanly when template creation fails. |
| Documentation | The new public test-support type and functions lacked item-level docs. | Low | Added concise public API documentation. |
| Performance measurement | The original benchmark only measured fresh migration cost and could not validate the template path. | Medium | Added `kg_schema_init_from_template` and documented the 72.68 ms to 4.06 ms local comparison. |
| Versioning | The workspace version had not reflected the shipped test-infrastructure change. | Low | Bumped the workspace package version to `0.160.1`. |
| Issue hygiene | The issue body still referred to 58 migrations although the workspace now has 60. | Low | Refreshed issue #535 to current reality. |
| Workspace hygiene | A full-suite run exposed two existing Obsidian accounting failures unrelated to the fixture. | Medium | Filed issue #608 with reproduction and context instead of changing the unrelated import implementation. |

## Verification

All review findings were actioned. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the targeted template, knowledge, server, and E2E tests, and `cargo test --workspace --no-fail-fast` pass except for the two pre-existing Obsidian failures tracked in #608.
