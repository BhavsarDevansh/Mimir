#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issue #518: the `docs/wiki/what-works-now.md` header
# stamp must track the workspace version. The file is the feature-level
# roadmap and its `**Version:**` line records the workspace release the file
# was last updated against, so a stale stamp makes the file read as current
# while showing an outdated version. The guard reads the version from the
# root `Cargo.toml` `[workspace.package]` block and fails when the stamp
# drifts from it.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$SCRIPT_DIR"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

workspace_version="$(awk '
    /^\[workspace\.package\]$/ { in_package = 1; next }
    /^\[/ { in_package = 0 }
    in_package && /^version = ".*"$/ {
        sub(/^version = "/, ""); sub(/"$/, ""); print; exit
    }
' Cargo.toml)"
[[ -n "$workspace_version" ]] || fail "could not read the workspace version from Cargo.toml"

stamp="$(awk '
    /^# What Works in Mimir Today$/ { in_header = 1; next }
    /^## / { in_header = 0 }
    in_header && /^> \*\*Version:\*\* / {
        sub(/^> \*\*Version:\*\* /, ""); print; exit
    }
' docs/wiki/what-works-now.md)"
[[ -n "$stamp" ]] || fail "could not read the **Version:** stamp from docs/wiki/what-works-now.md"

[[ "$stamp" == "$workspace_version" ]] || fail "docs/wiki/what-works-now.md stamp is '$stamp', want '$workspace_version'"

printf 'what-works-now-version_test.sh: all checks passed\n'
