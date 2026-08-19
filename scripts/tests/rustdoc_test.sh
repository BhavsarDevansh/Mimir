#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issues #276, #310, #337 and #348: mimir-connectors,
# mimir-knowledge, mimir-core and mimir-server rustdoc must build with zero
# warnings so broken intra-doc links are caught at review time.

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

# mimir-server: `--all-features` covers the connector backend features; the
# doc surface must stay warning-free (issue #348).
RUSTDOCFLAGS="-D warnings" cargo doc -p mimir-server --no-deps --all-features
