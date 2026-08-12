# E2E Testing Architecture

## Overview

The end-to-end test in `mimir/tests/e2e.rs` validates the full request lifecycle:

1. A temporary directory is created.
2. An isolated `config.toml` is written pointing the server and memory to the temp dir.
3. The server is started **in-process** via `mimir_server::start_server_with_llm_and_listener`, using a pre-bound [`TcpListener`] on an ephemeral port and a [`MockLlmClient`] injected as the LLM backend.
4. The test polls `/status` until the daemon reports healthy.
5. The real `mimir` CLI binary is spawned (`mimir ask --no-stream hello`) with `MIMIR_BASE_URL` set.
6. stdout is asserted to contain the mock response.
7. `mimir stop` is sent via the CLI binary and the in-process server task is asserted to exit within 5 seconds.

## Why In-Process Server + OS Process CLI?

The daemon runs in-process so tests can inject a [`MockLlmClient`] directly via the public `start_server_with_llm_and_listener` API. This avoids compiling test-only code into the production binary.

The CLI commands (`mimir ask`, `mimir stop`) are spawned as real OS processes via `env!("CARGO_BIN_EXE_mimir")` so the actual argument parsing, env handling, and HTTP client paths are exercised.

## Mock LLM Injection

The E2E test creates a [`MockLlmClient`] with deterministic responses and passes it to `start_server_with_llm_and_listener`. The production `mimir` binary never sees mock code; the mock backend lives only in the test process.

## Environment Variables Used

| Variable | Purpose |
|----------|---------|
| `MIMIR_BASE_URL` | Overrides `DEFAULT_BASE_URL` in all CLI commands. Cached with `LazyLock`. |
| `MIMIR_MEMORY_PATH` | Overrides the memory file path at runtime. |
| `HOME` | Isolates platform-specific home directories (critical on macOS where `dirs` ignores XDG). |
| `XDG_CONFIG_HOME` | Isolates config for spawned CLI processes. |
| `XDG_DATA_HOME` | Isolates data (DB, memory) for spawned CLI processes. |

## Extending the Harness

To add a new E2E scenario (e.g. streaming chat, memory viewing):

1. Reuse the same temp-dir + isolated-config pattern.
2. Start the server in-process with a pre-bound listener and injected mock backend.
3. Set `MIMIR_BASE_URL` on every CLI invocation.
4. Assert on captured stdout/stderr.

## Connector E2E (Phase 3 T1 / issue #206)

`mimir/tests/connector_e2e.rs` extends the same pattern to the connector framework. The shared `TestDaemon` fixture (`mimir/tests/common/mod.rs`) starts the daemon with the `mock-connector` feature so the `gmail/test` mock backend is registered, and `run_cli_json` runs a CLI subcommand and parses its JSON stdout (asserting success first).

The fact-ingestion tests configure the mock's `facts` knob via `mimir connector add --config-json`, then drive add → auth → resume → sync and verify the knowledge graph through the real HTTP surface:

- `mimir kb query <entity> --json` — fact presence, predicate/object, and confidence (Gmail reliability score = 0.85).
- `mimir kb show <fact_id> --json` — provenance: a `Connector` source with the instance id and `raw_reference`.
- `mimir connector status --json` — `sync_cursor` persistence and the derived per-instance `item_count`.

Corroboration is exercised end-to-end: a second instance emitting the same claim merges into the existing fact row (entity resolution), adds an independent source, and boosts confidence to 0.90; a plain re-sync of the first instance is asserted to be a re-statement no-op (no extra source, no further boost). The supervisor-level round trip (polling + push modes) lives in `mimir-connectors/tests/mock_ingestion_e2e.rs`.
