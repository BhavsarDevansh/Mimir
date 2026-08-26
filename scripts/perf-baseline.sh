#!/usr/bin/env bash
# Performance baseline runner for the Mimir test suite.
#
# Measures the full workspace test suite and reports the wall-clock total
# plus the slowest tests, so performance improvements can be tracked against
# a stable baseline. Prefers cargo-nextest for per-test timings; falls back
# to `cargo test --workspace` (wall time only) when nextest is unavailable.
#
# Usage:
#   scripts/perf-baseline.sh            # whole suite
#   scripts/perf-baseline.sh <filter>   # e.g. mimir-knowledge
#
# Set NEXTEST_BIN to a cargo-nextest binary not on PATH (e.g. a download).

set -euo pipefail

cd "$(dirname "$0")/.."

FILTER="${1:-}"
LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

nextest_bin="${NEXTEST_BIN:-}"
if [[ -z "$nextest_bin" ]] && command -v cargo-nextest >/dev/null 2>&1; then
    nextest_bin="$(command -v cargo-nextest)"
fi

if [[ -n "$nextest_bin" ]]; then
    echo "==> Running workspace tests with cargo-nextest ($nextest_bin)"
    start="$(date +%s.%N)"
    if [[ -n "$FILTER" ]]; then
        "$nextest_bin" nextest run -p "$FILTER" --status-level pass 2>&1 | tee "$LOG"
    else
        "$nextest_bin" nextest run --workspace --status-level pass 2>&1 | tee "$LOG"
    fi
    end="$(date +%s.%N)"

    echo
    echo "== Summary"
    grep -E "^     Summary" "$LOG" || true
    total="$(grep -oP 'PASS \[\s*\K[0-9.]+(?=s)' "$LOG" | awk '{ sum += $1 } END { print sum + 0 }')"
    awk -v s="$start" -v e="$end" -v t="${total:-0}" \
        'BEGIN { printf "wall time: %.1f s\nsum of per-test durations: %.1f s\n", e - s, t }'
    echo
    echo "== Slowest 25 tests"
    grep -oP 'PASS \[\s*\K[0-9.]+s\] \(\s*\d+/\d+\) .*' "$LOG" | sort -rn | head -25
else
    echo "== cargo-nextest not found; falling back to \`cargo test --workspace\` (wall time only)"
    if [[ -n "$FILTER" ]]; then
        time cargo test -p "$FILTER"
    else
        time cargo test --workspace
    fi
fi
