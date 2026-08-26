# Release Process

## Overview

Mimir has no CI/CD pipeline: a release is a local, one-command step. `scripts/new-release.sh` reads the version, takes the matching `## [VERSION]` section from `CHANGELOG.md` as the release body, creates an annotated tag, pushes it, and publishes a GitHub release via the `gh` CLI. The full release history lives on [GitHub Releases](https://github.com/BhavsarDevansh/Mimir/releases), with the most recent versions in `CHANGELOG.md` and everything before 0.152.0 archived in `CHANGELOG-archive.md`.

## Before releasing

1. Bump the version in the root `Cargo.toml` `[workspace.package]` table only; every workspace member inherits it via `version.workspace = true`.
2. Add a `## [x.y.z] — YYYY-MM-DD` section at the top of `CHANGELOG.md` summarising the change set, ending with a `Version bumped a.b.c → x.y.z (patch | minor | major)` line.
3. Update `docs/` and `docs/wiki/` and run the code review per AGENTS.md; the release script itself is unit-tested by `scripts/tests/new-release_test.sh`.

## Releasing

`scripts/new-release.sh [--dry-run] [--version VER] [VERSION]`:

- The version defaults to the root `Cargo.toml` version; `--version VER` or a positional VERSION overrides it.
- The script extracts the `## [VERSION]` section from `CHANGELOG.md` (override with `$CHANGELOG_FILE` and `$CARGO_MANIFEST`) and refuses to publish when the section is missing, the version is malformed, the tag `vVERSION` already exists locally or on `origin` at another commit, or the working tree is dirty.
- `--dry-run` validates everything and prints the plan (version, tag, notes size) plus a warning if the working tree is dirty, without touching the repository or GitHub.
- Publishing creates the annotated tag `vVERSION`, pushes it to the `origin` remote, and runs `gh release create vVERSION --title vVERSION --notes-file <section>`; `gh` must be installed and authenticated (`gh auth login`), and the working tree must be clean.
- Publication is resumable: if a run pushes the tag but `gh release create` fails, re-running the script validates the tag target and publishes the missing release instead of aborting; an already-published release is a no-op.

## Changelog maintenance

`CHANGELOG.md` keeps the most recent three version sections; older entries are archived into `CHANGELOG-archive.md` whenever the file grows. GitHub Releases is the canonical full history, so trimming the main changelog never loses a release note.
