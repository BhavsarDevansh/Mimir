#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issues #276, #310 and #337: mimir-connectors,
# mimir-knowledge and mimir-core rustdoc must build with zero warnings so
# broken intra-doc links are caught at review time.
#
# Scoped to these three crates for now; the remaining workspace crate with doc
# warnings is tracked in #348 (mimir-server). Widen this check once that lands.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$SCRIPT_DIR"

# `--all-features` covers the default feature set; `--no-default-features`
# guards the feature-gated link class (e.g. `crate::mock`) fixed in #276.
RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-connectors --no-deps --all-features
RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-connectors --no-deps --no-default-features

# mimir-knowledge has no feature flags; the default build is the full surface.
RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-knowledge --no-deps

# mimir-core's only feature is `mock-llm` (off by default); the default build
# is the primary surface and `--all-features` guards the feature-gated links.
RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-core --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-core --no-deps --all-features
