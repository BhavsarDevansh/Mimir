# Test Schema Templates

## Implementation

`mimir-test-support` is a development-only workspace crate that owns one pre-migrated SQLite template per test binary. On first use it runs the real knowledge-graph migrations against a temporary source database, atomically writes a clean schema-only database with `VACUUM INTO`, closes the source connection, and then keeps the template path in an asynchronous once-cell guarded by a mutex. `init_from_template` copies that template to the requested destination and opens it with the production `KnowledgeGraph::init` path.

## Rationale

The template preserves the production schema while moving repeated migration work out of the per-test path. A measured comparison on v0.160.1 showed 72.68 ms for a fresh 60-migration init and 4.06 ms for template copy plus init. The template is created from `KnowledgeGraph::init`, so the fixture cannot drift from the migration runner; migration and schema tests continue to call `KnowledgeGraph::init` directly so they still exercise every migration.

## Connections

`mimir-knowledge/tests/common/mod.rs` uses the fixture for `TestGraph`, `mimir-server/tests/common/mod.rs` uses it for the shared server `AppState`, and `mimir/tests/common/mod.rs` seeds the knowledge database before starting the in-process E2E daemon. The benchmark suite records both fresh and template setup costs under `kg_write_benchmarks`.

## Safety

The crate is linked only through `dev-dependencies`, so production binaries do not compile or ship it. It contains no `unsafe` code, no global environment mutation, and each copied database is inside the calling test's temporary directory.
