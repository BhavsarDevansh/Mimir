//! Baseline comparison helpers for memory benchmark reports.

use super::{BenchmarkReport, MetricName, PerformanceName};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetricDelta {
    pub name: String,
    pub baseline: f64,
    pub current: f64,
    pub delta: f64,
    pub direction: DeltaDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeltaDirection {
    Better,
    Worse,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BaselineComparison {
    pub quality: Vec<MetricDelta>,
    pub performance: Vec<MetricDelta>,
    pub regressions: Vec<MetricDelta>,
}

pub fn compare_baseline(
    current: &BenchmarkReport,
    baseline: &BenchmarkReport,
) -> BaselineComparison {
    let mut quality = Vec::new();
    for metric in [
        MetricName::RecallAt5,
        MetricName::PrecisionAt5,
        MetricName::ProvenanceAccuracy,
        MetricName::CitationFabricationRate,
        MetricName::TemporalCorrectness,
        MetricName::ConsolidationStability,
        MetricName::DedupPrecision,
        MetricName::PrivacyFalseAllowRate,
        MetricName::PrivacyFalseBlockRate,
    ] {
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
    for metric in [
        PerformanceName::RetrievalLatencyP95,
        PerformanceName::RetrievalLatencyP99,
        PerformanceName::IngestionThroughput,
        PerformanceName::MemoryIndexGrowth,
        PerformanceName::RenderedTokenOutput,
        PerformanceName::BenchmarkWallTime,
    ] {
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
