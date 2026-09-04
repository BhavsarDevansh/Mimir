//! Baseline comparison helpers for memory benchmark reports.

use super::{BenchmarkReport, PERFORMANCE_METRICS, QUALITY_METRICS};

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

/// Compares a current benchmark report against a saved baseline and records regressions.
pub fn compare_baseline(
    current: &BenchmarkReport,
    baseline: &BenchmarkReport,
) -> BaselineComparison {
    let mut quality = Vec::new();
    for metric in QUALITY_METRICS {
        let baseline_value = baseline
            .metrics
            .quality
            .get(metric.as_str())
            .copied()
            .unwrap_or_default();
        let current_value = current
            .metrics
            .quality
            .get(metric.as_str())
            .copied()
            .unwrap_or_default();
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
        let baseline_value = baseline
            .metrics
            .performance
            .get(metric.as_str())
            .copied()
            .unwrap_or_default();
        let current_value = current
            .metrics
            .performance
            .get(metric.as_str())
            .copied()
            .unwrap_or_default();
        let difference = current_value - baseline_value;
        let direction = if difference == 0.0 {
            DeltaDirection::Unchanged
        } else if (difference > 0.0) == metric.is_higher_better() {
            DeltaDirection::Better
        } else {
            DeltaDirection::Worse
        };
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

    BaselineComparison {
        quality,
        performance,
        regressions,
    }
}
