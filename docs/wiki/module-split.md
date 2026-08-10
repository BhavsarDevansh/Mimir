# Module Split (0.94.0)

## What changed

Mimir's source code has been reorganised so that every file has one specific job. Several files had grown very large over time — the biggest was over four thousand lines — which made them hard to navigate and review. They have been broken into smaller modules grouped by concern, and each crate's public interface is unchanged.

This is a code-organisation change only. Behaviour, configuration, the HTTP API, and the CLI are exactly the same as before. Your data, setup, and scripts keep working unchanged.

## Why it matters

Smaller, single-purpose files make the codebase easier to understand, review, test, and maintain. When a bug is reported, it is now much faster to find the exact module responsible. Each module documents its own responsibility, so new contributors (or a future you) can reason about one concern at a time instead of scrolling through thousands of lines.

## How the codebase is organised now

- Each subsystem lives in its own directory, e.g. connectors, calendar, email, secrets, knowledge-graph queries, and server routes.
- Within a directory, files are named after their job, e.g. `sync.rs`, `credentials.rs`, `query.rs`, `ranking.rs`.
- Large test suites were split the same way: tests are grouped by the feature they exercise, with shared helpers in a common test module.
- The public entry point of every crate still exports the same names, so nothing in the ecosystem (scripts, configuration, the HTTP API) needs to change.

## Where things live

- Connector supervision, calendar, email, rate limiting, geocoding, mock, photos, and secrets: `mimir-connectors/src/`.
- Knowledge-graph queries and the fact pipeline: `mimir-knowledge/src/`.
- Server routes, state, and shutdown handling: `mimir-server/src/`.
- Client library and wire types: `mimir-client/src/` and `mimir-api-types/src/`.

A detailed technical module map is in `docs/refactoring-module-split.md`.

## Best practices going forward

- Keep new code in the module that owns its concern; avoid growing files back into multi-thousand-line catch-alls.
- If a file starts mixing two jobs, split it into two files and re-export from the parent module so callers stay unchanged.
- Put unit tests next to the code they test, and integration suites in `tests/` grouped by feature.
- Run `cargo fmt`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` before committing.
