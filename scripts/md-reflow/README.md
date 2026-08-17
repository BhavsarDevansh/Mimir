# md-reflow

Reflows Markdown prose to the Mimir single-line standard (AGENTS.md "Finishing Work"): every prose paragraph and list-item continuation becomes a single flowing line, with blank lines only between blocks.

## Usage

Run from the repository root:

```bash
cargo run --manifest-path scripts/md-reflow/Cargo.toml -- --check   # report files that would change (exit 1 if any)
cargo run --manifest-path scripts/md-reflow/Cargo.toml -- --reflow   # reflow all .md files in place
cargo run --manifest-path scripts/md-reflow/Cargo.toml -- --survey  # list remaining wrapped regions
```

Pass explicit paths to limit the run, e.g. `--check docs/ README.md`.

If you pass no mode flag, the tool uses `--reflow` and writes files in place.

A conventional `--` separator ends flag parsing: every argument after it is treated as a path verbatim, so files whose names start with `--` can be processed, e.g. `md-reflow --check -- --weird.md`.

In `--check` mode the exit code is 1 when any file would change or could not be read, so unreadable files fail CI instead of being silently skipped.

## What it does

- Joins wrapped paragraphs and tight list items onto one line, preserving the exact source text (inline markup is never rewritten).
- Splits blockquote field-lists (every line starts with a `**Field:**` marker) so each entry is its own blockquote paragraph with a blank `>` line between entries.
- Leaves tables, fenced code blocks, nested lists, HTML blocks, and wrapped blockquote prose untouched — mixed field-list/wrapped blockquote regions are reported by `--survey` for manual restructuring.

## Verification

The reflow is content-preserving: a whitespace-collapsed comparison of a file before and after reflow is identical (blank `>` separator lines excluded). The `--check` mode is the enforcement entry point, wired into the repo's review-time regression guards as `scripts/tests/md-reflow_test.sh` (issue #294), so hard-wrap drift fails at review time.
