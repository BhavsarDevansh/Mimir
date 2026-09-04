//! Baseline comparison helpers for memory benchmark reports.

use super::{BenchmarkReport, PERFORMANCE_METRICS, PerformanceName, QUALITY_METRICS};

/// The absolute and directional change for one benchmark metric.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetricDelta {
    pub name: String,
    pub baseline: f64,
    pub current: f64,
    pub delta: f64,
    pub direction: DeltaDirection,
}

/// Whether a metric change improved, worsened, or did not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeltaDirection {
    Better,
    Worse,
    Unchanged,
}

/// The complete comparison between a current report and a saved baseline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BaselineComparison {
    pub quality: Vec<MetricDelta>,
    pub performance: Vec<MetricDelta>,
    pub regressions: Vec<MetricDelta>,
}

fn performance_direction(
    metric: PerformanceName,
    baseline: f64,
    difference: f64,
) -> DeltaDirection {
    if difference == 0.0 {
        return DeltaDirection::Unchanged;
    }
    let relative_tolerance = match metric {
        PerformanceName::RetrievalLatencyP95
        | PerformanceName::RetrievalLatencyP99
        | PerformanceName::IngestionThroughput
        | PerformanceName::BenchmarkWallTime => 0.30,
        PerformanceName::MemoryIndexSize => 0.01,
        PerformanceName::RenderedTokenOutput => 0.0,
    };
    let tolerance = relative_tolerance * baseline.abs();
    let deteriorated = if metric.is_higher_better() {
        difference < 0.0
    } else {
        difference > 0.0
    };
    if deteriorated && difference.abs() <= tolerance {
        return DeltaDirection::Unchanged;
    }
    if (difference > 0.0) == metric.is_higher_better() {
        DeltaDirection::Better
    } else {
        DeltaDirection::Worse
    }
}

/// Compares complete benchmark reports against a same-fixture baseline and records regressions.
///
/// Non-deterministic performance metrics use relative tolerances so ordinary variance is not
/// classified as a regression.
pub fn compare_baseline(
    current: &BenchmarkReport,
    baseline: &BenchmarkReport,
) -> Result<BaselineComparison, crate::KnowledgeError> {
    if current.fixture_version != baseline.fixture_version {
        return Err(crate::KnowledgeError::Validation(format!(
            "baseline fixture version mismatch: expected {}, got {}",
            current.fixture_version, baseline.fixture_version
        )));
    }
    let mut quality = Vec::new();
    for metric in QUALITY_METRICS {
        let Some(baseline_value) = baseline.metrics.quality.get(metric.as_str()).copied() else {
            return Err(crate::KnowledgeError::Validation(format!(
                "missing baseline metric: {}",
                metric.as_str()
            )));
        };
        let Some(current_value) = current.metrics.quality.get(metric.as_str()).copied() else {
            return Err(crate::KnowledgeError::Validation(format!(
                "missing current metric: {}",
                metric.as_str()
            )));
        };
        let difference = current_value - baseline_value;
        let direction = if difference == 0.0 {
            DeltaDirection::Unchanged
        } else if (difference > 0.0) != metric.is_lower_better() {
            DeltaDirection::Better
        } else {
            DeltaDirection::Worse
        };
        quality.push(MetricDelta {
            name: metric.as_str().to_string(),
            baseline: baseline_value,
            current: current_value,
            delta: difference,
            direction,
        });
    }

    let mut performance = Vec::new();
    for metric in PERFORMANCE_METRICS {
        let Some(baseline_value) = baseline.metrics.performance.get(metric.as_str()).copied()
        else {
            return Err(crate::KnowledgeError::Validation(format!(
                "missing baseline metric: {}",
                metric.as_str()
            )));
        };
        let Some(current_value) = current.metrics.performance.get(metric.as_str()).copied() else {
            return Err(crate::KnowledgeError::Validation(format!(
                "missing current metric: {}",
                metric.as_str()
            )));
        };
        let difference = current_value - baseline_value;
        let direction = performance_direction(*metric, baseline_value, difference);
        performance.push(MetricDelta {
            name: metric.as_str().to_string(),
            baseline: baseline_value,
            current: current_value,
            delta: difference,
            direction,
        });
    }

    let mut regressions = Vec::new();
    regressions.extend(
        quality
            .iter()
            .filter(|item| item.direction == DeltaDirection::Worse)
            .cloned(),
    );
    regressions.extend(
        performance
            .iter()
            .filter(|item| item.direction == DeltaDirection::Worse)
            .cloned(),
    );
    regressions.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(BaselineComparison {
        quality,
        performance,
        regressions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::MetricName;

    fn report(fixture_version: u32) -> BenchmarkReport {
        BenchmarkReport {
            schema_version: 1,
            fixture_version,
            generated_at: "2026-01-01T00:00:00Z".to_string(),
            metrics: Default::default(),
            violations: Vec::new(),
        }
    }

    #[test]
    fn compare_baseline_rejects_missing_metrics() {
        let current = report(1);
        let mut baseline = report(1);
        baseline
            .metrics
            .quality
            .insert(MetricName::RecallAt5.as_str().to_string(), 0.80);
        baseline
            .metrics
            .quality
            .remove(MetricName::RecallAt5.as_str());

        let error = compare_baseline(&current, &baseline)
            .expect_err("incomplete baseline must be rejected");
        assert!(error.to_string().contains("missing baseline metric"));
    }

    #[test]
    fn compare_baseline_applies_performance_tolerance() {
        let mut current = report(1);
        let mut baseline = report(1);
        let metric = PerformanceName::RetrievalLatencyP95;
        baseline
            .metrics
            .performance
            .insert(metric.as_str().to_string(), 1_000.0);
        current
            .metrics
            .performance
            .insert(metric.as_str().to_string(), 1_049.0);

        let comparison = compare_baseline(&current, &baseline).expect("complete reports");
        assert_eq!(
            comparison.performance[0].direction,
            DeltaDirection::Unchanged
        );
        assert!(comparison.regressions.is_empty());

        current
            .metrics
            .performance
            .insert(metric.as_str().to_string(), 1_500.0);

        let comparison = compare_baseline(&current, &baseline).expect("complete reports");
        assert_eq!(comparison.performance[0].direction, DeltaDirection::Worse);
        assert_eq!(comparison.regressions[0].name, metric.as_str());
    }

    #[test]
    fn compare_baseline_rejects_fixture_mismatch() {
        let comparison = compare_baseline(&report(1), &report(2)).expect_err("fixture mismatch");
        assert!(
            comparison
                .to_string()
                .contains("baseline fixture version mismatch")
        );
    }
}
