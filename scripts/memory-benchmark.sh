#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

args=()
if [[ $# -gt 0 ]]; then
    args=("$@")
fi

cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- "${args[@]}"
