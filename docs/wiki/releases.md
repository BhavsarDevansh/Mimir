# Releases

## What is a release

Every Mimir version is published as a GitHub Release with release notes on the [Releases page](https://github.com/BhavsarDevansh/Mimir/releases). The notes are written in `CHANGELOG.md` first and become the release body automatically — no build pipeline or CI is involved.

## Where to find release notes

- The GitHub Releases page — the canonical full history.
- `CHANGELOG.md` at the repository root — the most recent versions.
- `CHANGELOG-archive.md` — entries older than 0.152.0 that were trimmed from the main changelog.

## How versions are numbered

Mimir follows Semantic Versioning: patch releases (0.152.1 → 0.152.2) cover backwards-compatible bug fixes and documentation updates; minor releases (0.152.2 → 0.153.0) add backwards-compatible features; major releases (0.x → 1.0.0) change public interfaces such as the OpenAI-compatible chat API, configuration formats, or data models.

## Best practices

- Add a changelog entry for every change before the release, following the format of recent entries.
- Run `scripts/new-release.sh --dry-run` to preview what will be published before actually releasing.
- Publish a release as soon as the version changes instead of letting entries accumulate.
- Keep `CHANGELOG.md` short by archiving older sections into `CHANGELOG-archive.md` when it grows.
