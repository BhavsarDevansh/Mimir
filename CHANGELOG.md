# Changelog

Release notes for recent versions. Full history: [GitHub Releases](https://github.com/BhavsarDevansh/Mimir/releases) and `CHANGELOG-archive.md` (versions before 0.152.2).

## [0.152.6] — 2026-08-26

### Fix: docs/wiki/what-works-now.md version stamp tracks the workspace again (issue #518)

- The `docs/wiki/what-works-now.md` header `**Version:**` stamp is refreshed from 0.152.0 to 0.152.6, so the feature-level roadmap no longer reads as current while showing an outdated release (the stamp had not tracked the 0.152.1 → 0.152.5 releases since PR #513).
- New regression guard `scripts/tests/what-works-now-version_test.sh` (issue #518) pins the stamp to the root `Cargo.toml` workspace version, so the drift fails at review time instead of silently accumulating; `docs/workspace.md` documents the guard.
- Version bumped 0.152.5 → 0.152.6 (patch — documentation fix).

## [0.152.5] — 2026-08-26

### Fix: in-cycle OAuth forced refresh now persists the recovered auth state (issue #516)

- A connector whose health probe reports `AuthExpired` and whose one-shot forced refresh succeeds (issue #507 recovery) now flips the persisted `auth_state` back to `authenticated` in the same cycle, so `mimir connector status` reflects the recovery immediately instead of staying `expired` until the runner is respawned or credentials are re-ingested.
- The cycle-recovery test now starts from a stale `Expired` row and asserts the persisted state becomes `Authenticated` after the forced refresh; docs updated (`docs/connectors-framework.md`, `docs/wiki/connectors.md`).
- Version bumped 0.152.4 → 0.152.5 (patch — bugfix).

## [0.152.4] — 2026-08-26

### Fix: md-reflow guard passes on docs/obsidian-export-import.md (issue #514)

- `docs/obsidian-export-import.md` now satisfies the AGENTS.md single-line prose standard: the blockquote field-list at the top (`> **Issue:** #62` / `> **Phase:** ...`) gained the blank `>` separator between entries that `scripts/md-reflow` requires, so `scripts/tests/md-reflow_test.sh` (issue #294) is green again.
- Issue #514 body refreshed with the accurate diagnosis: the prose paragraphs were already single-line, and the field-list separator was the remaining violation.
- Archive-boundary references in `docs/release-process.md` and `docs/wiki/releases.md` updated to the new 0.152.2 cutoff.
- Version bumped 0.152.3 → 0.152.4 (patch — documentation fix).

## [0.152.3] — 2026-08-26

### Fix: PR #515 review — release script validation and resumable publication

- `scripts/new-release.sh` now matches the `## [VERSION]` changelog heading as literal text, so a version containing dots can never extract a look-alike section, and the script validates the origin tag state (`origin` must not already carry `vVERSION` at another commit) plus `gh auth status` before creating or pushing anything.
- A real release now requires a clean working tree (the dirty-tree warning is kept for `--dry-run` only), and publication is resumable: if a run pushed the tag but `gh release create` failed, re-running the script verifies the tag target and publishes the missing release instead of aborting, and an already-published release is a no-op.
- `scripts/tests/new-release_test.sh` gained regression coverage: literal heading matching with wildcard-substitution characters, remote-tag conflicts, dirty-tree rejection, missing `gh` authentication, and the failed-publication resume path via a `gh` test double; docs updated (`docs/release-process.md`, `docs/wiki/releases.md`, `docs/workspace.md`).
- Version bumped 0.152.2 → 0.152.3 (patch — bugfixes to the release tooling).

## [0.152.2] — 2026-08-26

### Tooling: one-command GitHub releases and a trimmed changelog

- New `scripts/new-release.sh` publishes the current version as a GitHub release with no CI/CD: the version defaults to the root `Cargo.toml` `[workspace.package]` version (or `--version VER` / a positional VERSION), the matching `## [VERSION]` section of `CHANGELOG.md` becomes the release body, `--dry-run` validates and prints the plan without publishing, an already-existing tag or a missing changelog section aborts the release, and publishing is an annotated `git tag -a vX.Y.Z`, `git push origin`, and `gh release create --notes-file` — with a warning when the working tree is dirty.
- `CHANGELOG.md` is trimmed to the three most recent sections; everything older now lives in `CHANGELOG-archive.md`, and every release is published to GitHub Releases so the history stays discoverable without a 3,600-line file.
- New regression guard `scripts/tests/new-release_test.sh` pins version detection, changelog section extraction, tag-existence checks, and the dry-run validation paths; docs updated (`docs/release-process.md`, `docs/wiki/releases.md`, `docs/workspace.md`).
- Version bumped 0.152.1 → 0.152.2 (patch — tooling and documentation).
