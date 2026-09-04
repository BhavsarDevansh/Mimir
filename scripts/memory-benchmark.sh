#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- "$@"
