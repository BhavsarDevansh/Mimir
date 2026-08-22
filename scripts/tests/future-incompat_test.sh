#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issue #446: no dependency may emit rustc future-incompat
# warnings (code that "will be rejected by a future version of Rust"), because a
# future toolchain turns them into hard build errors. proc-macro-error2 v2.0.1
# (pulled by tabled 0.21 via tabled_derive 0.11) triggered E0365; it is abandoned
# upstream (last crates.io release 2024-09) with no fixed release, so the
# workspace vendors a patched copy at vendor/proc-macro-error2 pinned through
# [patch.crates-io]. Dropping that patch, or a new dependency introducing a
# similar warning, fails this guard at review time.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# A warm target/ directory can skip rustc for unchanged dependencies, hiding
# the diagnostic; build in a fresh target directory so every dependency is
# compiled, and inspect Cargo's stored report as well.
TARGET_DIR="$(mktemp -d)"
trap 'rm -rf "$TARGET_DIR"' EXIT
export CARGO_TARGET_DIR="$TARGET_DIR"

OUTPUT="$(cargo clippy --workspace --all-targets --all-features --future-incompat-report 2>&1)" || {
  echo "$OUTPUT"
  exit 1
}

# Cargo stores the future-incompat report separately; read it from the same
# target directory. `cargo report` exits 101 when no report exists, which is
# the clean case; any other failure must fail the guard instead of hiding
# warnings.
REPORT="$(cargo report future-incompatibilities 2>&1)" || {
  case "$REPORT" in
    *"no reports are currently available"*) REPORT="" ;;
    *) echo "$REPORT" >&2
       exit 1 ;;
  esac
}

PATTERN="will be rejected by a future version of Rust|will become (an error|a hard error) in a future release"

if grep -Eq "$PATTERN" <<<"$OUTPUT" || grep -Eq "$PATTERN" <<<"$REPORT"; then
  echo "error: dependency future-incompat warnings detected:" >&2
  { echo "$OUTPUT"; echo "$REPORT"; } | grep -E -B2 -A2 "$PATTERN" >&2 || true
  echo "Patch or upgrade the offending crate (see vendor/proc-macro-error2 for the existing fix)." >&2
  exit 1
fi

echo "no dependency future-incompat warnings"
