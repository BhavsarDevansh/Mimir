# Changelog

Release notes for recent versions. Full history: [GitHub Releases](https://github.com/BhavsarDevansh/Mimir/releases) and `CHANGELOG-archive.md` (versions before 0.152.1).

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

## [0.152.1] — 2026-08-26

### Fix: PR #513 review round 2 — recurrence constraints, range time zones, and series retirement (issue #474)

- The events-subsystem occurrence engine now evaluates the stored `RRULE` day/month constraints instead of advancing only by kind/interval: `BYDAY` selects the weekdays of a weekly series (multi-day weekly events advance to the next constrained weekday, respecting `INTERVAL` weeks), `BYMONTHDAY` the day of an absolute monthly/yearly series, `BYMONTH` the month of a yearly series, and `BYDAY` + `BYSETPOS` the Nth weekday of a relative monthly/yearly series (including `last`). `next_occurrence` takes the raw rule, and occurrence-level tests cover multi-day weekly, fortnightly multi-day, absolute/relative monthly, and absolute/relative yearly patterns.
- The Graph `endDate` range now preserves `recurrenceTimeZone`: `UNTIL` is the inclusive local end-of-day (`23:59:59`) in the range's time zone (falling back to the event time zone, then UTC), converted to UTC — a zone ahead of UTC no longer leaks the next local date into the series and a zone behind UTC no longer truncates the last local day.
- The upcoming scan retires a recurring overlay when its series ends: when `next_occurrence` returns `None` (the next occurrence would fall past `recurrence_until`, or the rule no longer yields one), the overlay transitions to `Completed` so the scan stops selecting it on every cycle and it never surfaces as overdue. Regression tests cover a past final occurrence and rule-driven advancement through the scan.
- Docs updated (`docs/events-reminders.md`, `docs/calendar-connector.md`, `docs/wiki/calendar-connector.md`, `Mimir-Implementation-Context.md`).
- Version bumped 0.152.0 → 0.152.1 (patch — backwards-compatible bugfixes).
