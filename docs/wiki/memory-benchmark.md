# Memory Benchmark

Mimir has a local memory benchmark harness that checks memory quality and performance without using the LLM or network. Run it before changing ranking, deduplication, temporal handling, provenance, privacy, or retrieval.

## What It Checks

The harness creates deterministic fixture data, ingests it through the normal knowledge graph pipeline, builds the condensed memory view, and reports recall and precision at the top five, provenance and citation accuracy, temporal correctness, consolidation stability, dedup precision, privacy false-allow/false-block rates, retrieval latency p95/p99, ingestion throughput, memory/index size, token output estimate, and benchmark wall time.

Duplicate event evidence is included so the same object under a different source must consolidate instead of becoming a separate disputed fact.

## How To Run It

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark
```

Or use the convenience script:

```bash
scripts/memory-benchmark.sh
```

Save a local baseline:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- --save-baseline /tmp/memory-baseline.json
```

Baseline files are checked for schema version and complete metrics before comparison.

Compare a later change against that baseline:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- --baseline /tmp/memory-baseline.json --output /tmp/memory-benchmark.json
```

Use `--scale` to ingest more filler facts when testing growth:

```bash
cargo bench -p mimir-knowledge --features test-benchmark --bench memory_benchmark -- --scale 10
```

## Interpreting Results

Higher is better for recall, precision, provenance accuracy, temporal correctness, consolidation stability, dedup precision, and ingestion throughput. Lower is better for citation fabrication, privacy false-allow/false-block rates, latency, index size, token output, and wall time.

The default output is JSON and includes named metrics plus any threshold violations. Quality thresholds are built into the harness. Performance values depend on hardware, so save a baseline locally before optimizing and compare on the same machine.

The runner writes baseline and output files even when it exits with a failure, so a failed run can still be inspected or attached to an issue.
