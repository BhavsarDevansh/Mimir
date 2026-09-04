# Memory Benchmark Harness

## Overview

`mimir-knowledge` includes a deterministic memory benchmark harness behind the off-by-default `test-benchmark` feature. It exercises the current knowledge graph: normalized fact ingestion, corroboration/supersession, memory ranking, deterministic rendering, provenance, temporal bounds, and the sensitivity gate. The harness has no LLM or network dependency.

The runner emits one JSON report to stdout with quality and performance metrics. It can save a local baseline, compare against a baseline, and fail on budget violations or regressions. Baseline comparison output and operational errors go to stderr so consumers can parse the report as a single JSON document. Performance values are machine-specific, so committed baselines should not be treated as universal performance expectations, and baseline comparisons apply relative tolerances to non-deterministic performance metrics.

The fixture bank also exercises fact-overlap semantics. `has_event` is treated as a multi-valued predicate, so overlap candidates are restricted to the same event and unrelated events cannot dispute one another. This lets duplicate evidence reach corroboration instead of creating a separate disputed fact. Single-valued predicates retain cross-object contradiction semantics.

## Running

Run the default suite with:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark
```

Or use the convenience script:

```bash
scripts/memory-benchmark.sh
```

Save a baseline:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- --save-baseline /tmp/memory-baseline.json
```

Baselines are validated for the current schema, matching fixture version, and all metric fields before comparison.

Compare a later change:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- --baseline /tmp/memory-baseline.json --output /tmp/memory-benchmark.json
```

Scale the fixture bank:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- --scale 10
```

## Quality Metrics

| Metric | Meaning |
|--------|---------|
| `recall_at_5` | Fraction of expected top facts present in the top five returned memory items. |
| `precision_at_5` | Fraction of the top five returned memory items that are expected facts. |
| `provenance_accuracy` | Fraction of returned facts whose stored source type matches the fixture expectation. |
| `citation_fabrication_rate` | Fraction of returned facts whose current provenance reference does not match the fixture expectation. This is the conservative pre-#581 interpretation. |
| `temporal_correctness` | Fraction of returned facts with exact fixture-expected validity bounds. |
| `consolidation_stability` | Fraction of facts with the same database ID after re-ingesting the deterministic fixture bank. |
| `dedup_precision` | Fraction of declared duplicate pairs that consolidate to the same fact. |
| `privacy_false_allow_rate` | Fraction of sensitive fixtures incorrectly exposed in the returned schema. |
| `privacy_false_block_rate` | Fraction of non-sensitive fixtures incorrectly placed in confirmation. |

## Performance Metrics

| Metric | Meaning |
|--------|---------|
| `retrieval_latency_p95_us` | 95th percentile latency across 100 deterministic memory-schema samples. |
| `retrieval_latency_p99_us` | 99th percentile latency across 100 deterministic memory-schema samples. |
| `ingestion_throughput_facts_per_second` | Facts accepted by the normalized ingestion path per second. |
| `memory_index_size_bytes` | SQLite database file size after ingestion. |
| `rendered_token_output_estimate` | Deterministic character estimate divided by four. |
| `benchmark_wall_time_ms` | Total harness wall time. |

## Fixture Coverage

The initial fixture bank covers identity, preferences, relationships, future/recurring/timezone/overdue events, photo metadata, assistant state, vision-shaped data, long-horizon history, duplicate evidence, conflicts, and sensitive positives/negatives. Future adapters can add fixture categories and metric inputs without changing the report shape.

## Budgets

`BenchmarkConfig` contains default quality thresholds and performance budgets. The runner fails when a quality metric is below its minimum, a rate is above its maximum, or a performance value breaches its budget. This provides a local gate before CI/CD is introduced. Baseline comparisons also apply relative tolerances to latency, throughput, wall time, and index-size deterioration so normal machine variance does not fail the run. The runner writes `--save-baseline` and `--output` reports before exiting on a violation or baseline regression.
