#![cfg(feature = "test-benchmark")]

use mimir_knowledge::benchmark::{
    BenchmarkConfig, BenchmarkMetrics, BenchmarkReport, MetricName, PerformanceName,
    ViolationMetric, compare_baseline, generate_fixture_bank, load_baseline, run_memory_benchmark,
    save_baseline,
};
use mimir_knowledge::models::enums::RecurrenceType;

#[tokio::test]
async fn benchmark_saves_and_loads_baseline() {
    let config = BenchmarkConfig::default();
    let report = run_memory_benchmark(&config).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("baseline.json");
    save_baseline(&report, &path).await.unwrap();
    let loaded = load_baseline(&path).await.unwrap();
    assert_eq!(report, loaded);
}

#[test]
fn baseline_comparison_flags_regressions() {
    let config = BenchmarkConfig::default();
    let mut current = BenchmarkReport {
        schema_version: 1,
        fixture_version: 1,
        seed: config.seed,
        generated_at: "2026-01-01T00:00:00Z".to_string(),
        metrics: BenchmarkMetrics::default(),
        violations: Vec::new(),
    };
    let mut baseline = current.clone();
    baseline
        .metrics
        .quality
        .insert(MetricName::RecallAt5.as_str().to_string(), 1.0);
    current.metrics.quality.insert(
        MetricName::CitationFabricationRate.as_str().to_string(),
        1.0,
    );
    baseline.metrics.performance.insert(
        PerformanceName::IngestionThroughput.as_str().to_string(),
        10.0,
    );
    baseline.metrics.performance.insert(
        PerformanceName::RetrievalLatencyP95.as_str().to_string(),
        1.0,
    );
    let comparison = compare_baseline(&current, &baseline);
    assert!(
        comparison
            .regressions
            .iter()
            .any(|delta| delta.name == "recall_at_5")
    );
    assert!(
        comparison
            .regressions
            .iter()
            .any(|delta| delta.name == "citation_fabrication_rate")
    );
    assert!(
        comparison
            .regressions
            .iter()
            .any(|delta| delta.name == "ingestion_throughput_facts_per_second")
    );
}

#[tokio::test]
async fn benchmark_reports_all_named_metrics() {
    let config = BenchmarkConfig::default();
    let report = run_memory_benchmark(&config).await.unwrap();
    let quality = report.metrics.quality;

    for name in [
        "recall_at_5",
        "precision_at_5",
        "provenance_accuracy",
        "citation_fabrication_rate",
        "temporal_correctness",
        "consolidation_stability",
        "dedup_precision",
        "privacy_false_allow_rate",
        "privacy_false_block_rate",
    ] {
        assert!(quality.contains_key(name), "missing quality metric {name}");
    }

    for name in [
        "retrieval_latency_p95_us",
        "retrieval_latency_p99_us",
        "ingestion_throughput_facts_per_second",
        "memory_index_growth_bytes",
        "rendered_token_output_estimate",
        "benchmark_wall_time_ms",
    ] {
        assert!(
            report.metrics.performance.contains_key(name),
            "missing performance metric {name}"
        );
    }
}

#[tokio::test]
async fn benchmark_respects_configured_thresholds() {
    let mut config = BenchmarkConfig::default();
    config.thresholds.insert(MetricName::RecallAt5, 2.0);
    let report = run_memory_benchmark(&config).await.unwrap();
    assert!(!report.violations.is_empty());
    assert!(
        report
            .violations
            .iter()
            .any(|violation| violation.metric == ViolationMetric::Quality(MetricName::RecallAt5))
    );
}

#[tokio::test]
async fn benchmark_reports_performance_budget_violations() {
    let mut config = BenchmarkConfig::default();
    config
        .performance_budgets
        .insert(PerformanceName::BenchmarkWallTime, 0.0);
    let report = run_memory_benchmark(&config).await.unwrap();
    let violation = report
        .violations
        .iter()
        .find(|violation| {
            violation.metric == ViolationMetric::Performance(PerformanceName::BenchmarkWallTime)
        })
        .expect("wall-time budget violation");
    assert_eq!(
        violation.value,
        report
            .metrics
            .performance
            .get(PerformanceName::BenchmarkWallTime.as_str())
            .copied()
            .unwrap_or_default()
    );
}

#[test]
fn generated_fixtures_are_deterministic() {
    let config = BenchmarkConfig::default();
    let first = generate_fixture_bank(&config).unwrap();
    let second = generate_fixture_bank(&config).unwrap();
    assert_eq!(first, second);
}

#[test]
fn recurring_event_fixture_has_weekly_recurrence() {
    let config = BenchmarkConfig::default();
    let bank = generate_fixture_bank(&config).unwrap();
    let fact = bank
        .facts
        .iter()
        .find(|fact| fact.id == "calendar-recurring")
        .expect("recurring event fixture");
    assert_eq!(fact.recurrence, RecurrenceType::Weekly);
    assert_eq!(fact.recurrence_rule.as_deref(), Some("FREQ=WEEKLY"));
}

#[test]
fn report_serialises_to_machine_readable_json() {
    let config = BenchmarkConfig::default();
    let report = BenchmarkReport {
        schema_version: 1,
        fixture_version: 1,
        seed: config.seed,
        generated_at: "2026-01-01T00:00:00Z".to_string(),
        metrics: BenchmarkMetrics::default(),
        violations: Vec::new(),
    };
    let json = serde_json::to_string(&report).unwrap();
    let parsed: BenchmarkReport = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.schema_version, 1);
    assert_eq!(parsed.fixture_version, 1);
}
