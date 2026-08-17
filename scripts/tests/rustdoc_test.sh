#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issue #276: mimir-connectors rustdoc must build with
# zero warnings so broken intra-doc links are caught at review time.
#
# Scoped to mimir-connectors for now; the remaining workspace crates with doc
# warnings are tracked in #310 (mimir-knowledge), #337 (mimir-core), and #348
# (mimir-server). Widen this check once those land.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$SCRIPT_DIR"

RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-connectors --no-deps --all-features
