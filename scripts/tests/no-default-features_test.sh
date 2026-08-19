#!/usr/bin/env bash
set -euo pipefail

# Regression guard for issue #277: every supported mimir-connectors feature
# combination must compile with --no-default-features --all-targets, so the
# integration-test fixtures stay feature-gated and the framework/mock-only
# build remains usable. The default feature set is covered by the regular
# workspace test run.
#
# Every supported combination must also be warning-free (issues #342, #351):
# the shared `connector_fact` constructor is used only by feature-gated
# backends (so the `fact` module is cfg-gated to match), and the OAuth
# refresh helpers are only called by the Calendar/Email backends (so they are
# cfg-gated to their callers, keeping the `oauth`-only combination clean).
# RUSTFLAGS="-D warnings" turns any dead-code or unused-import warning into a
# hard failure, so these supported build configurations cannot regress.

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
  RUSTFLAGS="-D warnings" cargo "${args[@]}"
done
