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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$SCRIPT_DIR"

OUTPUT="$(cargo clippy --workspace --all-targets --all-features 2>&1)" || {
  echo "$OUTPUT"
  exit 1
}

if grep -q "will be rejected by a future version of Rust" <<<"$OUTPUT"; then
  echo "error: dependency future-incompat warnings detected:" >&2
  grep -B2 -A2 "will be rejected by a future version of Rust" <<<"$OUTPUT" >&2 || true
  echo "Patch or upgrade the offending crate (see vendor/proc-macro-error2 for the existing fix)." >&2
  exit 1
fi

echo "no dependency future-incompat warnings"
