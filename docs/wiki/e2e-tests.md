# Running E2E Tests Locally

## Quick Start

```bash
cargo test --test e2e
```

This builds the real `mimir` binary, starts the server in-process with a mock LLM backend on a random port, and verifies that `mimir ask --no-stream hello` returns the mock response.

## What the Test Covers

- **Config isolation**: The test creates a temporary directory with its own `config.toml` and SQLite database.
- **Full database isolation**: The in-process daemon is pointed at tempdir paths for all three SQLite databases (`context.db`, `knowledge.db`, `jobs.db`) via `context.db_path`, `knowledge.db_path`, and `scheduler.db_path`, so the suite never touches or migrates the developer's real `~/.local/share/mimir/*.db` files (issue #233).
- **Daemon lifecycle**: Starts the server in-process via `mimir_server::start_server_with_llm_and_listener`, polls `/status`, sends `mimir ask` via the real binary, and gracefully stops with `mimir stop`.
- **Mock LLM round-trip**: Asserts that the CLI correctly forwards the user query and prints the mock assistant response.

## Requirements

- Rust toolchain (stable)
- `cargo` (the test uses `env!("CARGO_BIN_EXE_mimir")` to locate the CLI binary)
- Local loopback networking (`127.0.0.1` must be available)

## Troubleshooting

### Test hangs or times out

- Check for orphaned `mimir start` processes: `pkill -f 'mimir start'`
- Ensure port `0` ephemeral binding is allowed by your OS.

### "daemon did not become ready within 10 seconds"

- The server logs its bound address. If the port is in use, try running the test again.
