#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issue #294: every repo `.md` file must keep the
# AGENTS.md single-line prose standard (flowing single-line paragraphs and
# list items, blank lines only between blocks). `md-reflow --check` reports
# every file whose prose would be reflowed and exits 1 when any file would
# change or could not be read, so hard-wrap drift fails at review time.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$SCRIPT_DIR"

cargo run --quiet --manifest-path scripts/md-reflow/Cargo.toml -- --check
