#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issue #277: every supported mimir-connectors feature
# combination must compile with --no-default-features --all-targets, so the
# integration-test fixtures stay feature-gated and the framework/mock-only
# build remains usable. The default feature set is covered by the regular
# workspace test run.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$SCRIPT_DIR"

# Each backend/test-harness feature is checked in isolation, plus the
# calendar + mock combination that exercises the shared tests/common module
# from both sides.
for features in "" "oauth" "calendar" "photos" "gmail" "test-mock-connector" "test-mock-oauth" "test-utils" "calendar,test-mock-connector"; do
  echo "checking --no-default-features --features '${features:-<none>}'"
  args=(check -p mimir-connectors --all-targets --no-default-features)
  if [[ -n "$features" ]]; then
    args+=(--features "$features")
  fi
  cargo "${args[@]}"
done
