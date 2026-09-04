//! Deterministic memory benchmark fixtures, metrics, and threshold reporting.

pub mod comparison;

pub use comparison::{BaselineComparison, DeltaDirection, MetricDelta, compare_baseline};

use std::collections::HashMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use tempfile::TempDir;

use crate::clock::MockClock;
use crate::models::entity::EntityType;
use crate::models::enums::RecurrenceType;
use crate::models::source::{ExtractionMethod, SourceType};
use crate::normalize::{NormalizedFact, Provenance, normalize_and_insert};

const QUALITY_METRICS: &[MetricName] = &[
    MetricName::RecallAt5,
    MetricName::PrecisionAt5,
    MetricName::ProvenanceAccuracy,
    MetricName::CitationFabricationRate,
    MetricName::TemporalCorrectness,
    MetricName::ConsolidationStability,
    MetricName::DedupPrecision,
    MetricName::PrivacyFalseAllowRate,
    MetricName::PrivacyFalseBlockRate,
];

const PERFORMANCE_METRICS: &[PerformanceName] = &[
    PerformanceName::RetrievalLatencyP95,
    PerformanceName::RetrievalLatencyP99,
    PerformanceName::IngestionThroughput,
    PerformanceName::MemoryIndexSize,
    PerformanceName::RenderedTokenOutput,
    PerformanceName::BenchmarkWallTime,
];

/// Quality metric names used by the deterministic memory benchmark report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MetricName {
    RecallAt5,
    PrecisionAt5,
    ProvenanceAccuracy,
    CitationFabricationRate,
    TemporalCorrectness,
    ConsolidationStability,
    DedupPrecision,
    PrivacyFalseAllowRate,
    PrivacyFalseBlockRate,
}

impl MetricName {
    /// Returns the stable JSON field name for this metric.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecallAt5 => "recall_at_5",
            Self::PrecisionAt5 => "precision_at_5",
            Self::ProvenanceAccuracy => "provenance_accuracy",
            Self::CitationFabricationRate => "citation_fabrication_rate",
            Self::TemporalCorrectness => "temporal_correctness",
            Self::ConsolidationStability => "consolidation_stability",
            Self::DedupPrecision => "dedup_precision",
            Self::PrivacyFalseAllowRate => "privacy_false_allow_rate",
            Self::PrivacyFalseBlockRate => "privacy_false_block_rate",
        }
    }

    pub fn is_lower_better(self) -> bool {
        matches!(
            self,
            Self::CitationFabricationRate
                | Self::PrivacyFalseAllowRate
                | Self::PrivacyFalseBlockRate
        )
    }
}

/// Performance metric names used by the deterministic memory benchmark report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PerformanceName {
    RetrievalLatencyP95,
    RetrievalLatencyP99,
    IngestionThroughput,
    MemoryIndexSize,
    RenderedTokenOutput,
    BenchmarkWallTime,
}

impl PerformanceName {
    /// Returns the stable JSON field name for this metric.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetrievalLatencyP95 => "retrieval_latency_p95_us",
            Self::RetrievalLatencyP99 => "retrieval_latency_p99_us",
            Self::IngestionThroughput => "ingestion_throughput_facts_per_second",
            Self::MemoryIndexSize => "memory_index_size_bytes",
            Self::RenderedTokenOutput => "rendered_token_output_estimate",
            Self::BenchmarkWallTime => "benchmark_wall_time_ms",
        }
    }

    pub fn is_higher_better(self) -> bool {
        self == Self::IngestionThroughput
    }
}

/// Inputs for the deterministic memory benchmark runner.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkConfig {
    pub fixture_version: u32,
    pub memory_budget: usize,
    pub min_confidence: f32,
    pub scale_multiplier: usize,
    pub thresholds: HashMap<MetricName, f64>,
    pub performance_budgets: HashMap<PerformanceName, f64>,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        let thresholds = [
            (MetricName::RecallAt5, 0.50),
            (MetricName::PrecisionAt5, 0.50),
            (MetricName::ProvenanceAccuracy, 0.50),
            (MetricName::CitationFabricationRate, 1.00),
            (MetricName::TemporalCorrectness, 0.50),
            (MetricName::ConsolidationStability, 0.50),
            (MetricName::DedupPrecision, 0.50),
            (MetricName::PrivacyFalseAllowRate, 1.00),
            (MetricName::PrivacyFalseBlockRate, 1.00),
        ]
        .into_iter()
        .collect();
        let performance_budgets = [
            (PerformanceName::RetrievalLatencyP95, 100_000.0),
            (PerformanceName::RetrievalLatencyP99, 250_000.0),
            (PerformanceName::IngestionThroughput, 1.0),
            (PerformanceName::MemoryIndexSize, 10_000_000.0),
            (PerformanceName::RenderedTokenOutput, 10_000.0),
            (PerformanceName::BenchmarkWallTime, 30_000.0),
        ]
        .into_iter()
        .collect();
        Self {
            fixture_version: 1,
            memory_budget: 2_500,
            min_confidence: 0.50,
            scale_multiplier: 1,
            thresholds,
            performance_budgets,
        }
    }
}

/// A declarative expectation for one fact in the benchmark fixture bank.
#[derive(Debug, Clone, PartialEq)]
pub struct FixtureFact {
    pub id: String,
    pub domain: &'static str,
    pub subject: String,
    pub predicate: &'static str,
    pub object: String,
    pub object_is_entity: bool,
    pub object_type: Option<&'static str>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub source_type: SourceType,
    pub category_ids: Vec<i32>,
    pub is_sensitive: bool,
    pub raw_reference: Option<String>,
    pub requires_user_action: bool,
    pub recurrence: RecurrenceType,
    pub recurrence_rule: Option<String>,
    pub expected_relevant: bool,
    pub expected_sensitive_allowed: bool,
    pub duplicate_of: Option<String>,
}

/// A declarative memory-query expectation for the benchmark fixture bank.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FixtureQuery {
    pub id: String,
    pub query: String,
    pub expected_fact_ids: Vec<String>,
    pub expected_top_fact_ids: Vec<String>,
}

/// Deterministic fixtures used by the memory benchmark runner.
#[derive(Debug, Clone, PartialEq)]
pub struct FixtureBank {
    pub fixture_version: u32,
    pub facts: Vec<FixtureFact>,
    pub queries: Vec<FixtureQuery>,
}

/// A quality or performance threshold that the benchmark report violated.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Violation {
    pub metric: ViolationMetric,
    pub value: f64,
    pub threshold: f64,
    pub kind: ViolationKind,
}

/// The comparison direction for a benchmark threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViolationKind {
    BelowMinimum,
    AboveMaximum,
}

/// A benchmark metric that can violate a configured threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViolationMetric {
    Quality(MetricName),
    Performance(PerformanceName),
}

/// Quality and performance measurements for one benchmark run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkMetrics {
    pub quality: HashMap<String, f64>,
    pub performance: HashMap<String, f64>,
}

impl Default for BenchmarkMetrics {
    fn default() -> Self {
        let mut quality = HashMap::new();
        for metric in QUALITY_METRICS {
            quality.insert(metric.as_str().to_string(), 0.0);
        }
        let mut performance = HashMap::new();
        for metric in PERFORMANCE_METRICS {
            performance.insert(metric.as_str().to_string(), 0.0);
        }
        Self {
            quality,
            performance,
        }
    }
}

/// The machine-readable result of a deterministic memory benchmark run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub fixture_version: u32,
    pub generated_at: String,
    pub metrics: BenchmarkMetrics,
    pub violations: Vec<Violation>,
}

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
}

/// Creates a complete entity-backed benchmark fixture.
#[allow(clippy::too_many_arguments)]
fn fact(
    id: &str,
    domain: &'static str,
    predicate: &'static str,
    object: &str,
    source_type: SourceType,
    category_ids: &[i32],
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> FixtureFact {
    FixtureFact {
        id: id.to_string(),
        domain,
        subject: "Devansh".to_string(),
        predicate,
        object: object.to_string(),
        object_is_entity: matches!(
            predicate,
            "has_partner" | "has_event" | "took_photo_at" | "resides_in" | "visited"
        ),
        object_type: if matches!(predicate, "prefers" | "has_medical_condition") {
            None
        } else if matches!(predicate, "has_event") {
            Some("Event")
        } else if matches!(predicate, "has_partner") {
            Some("Person")
        } else if matches!(predicate, "took_photo_at" | "resides_in" | "visited") {
            Some("Place")
        } else {
            None
        },
        valid_from,
        valid_until,
        source_type,
        category_ids: category_ids.to_vec(),
        is_sensitive: predicate == "health_condition",
        raw_reference: Some(format!("{domain}:{id}")),
        requires_user_action: false,
        recurrence: RecurrenceType::None,
        recurrence_rule: None,
        expected_relevant: true,
        expected_sensitive_allowed: false,
        duplicate_of: None,
    }
}

/// Creates a complete literal-valued benchmark fixture.
fn literal_fact(
    id: &str,
    domain: &'static str,
    predicate: &'static str,
    object: &str,
    source_type: SourceType,
    category_ids: &[i32],
) -> FixtureFact {
    FixtureFact {
        id: id.to_string(),
        domain,
        subject: "Devansh".to_string(),
        predicate,
        object: object.to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        source_type,
        category_ids: category_ids.to_vec(),
        is_sensitive: predicate == "health_condition",
        raw_reference: Some(format!("{domain}:{id}")),
        requires_user_action: false,
        recurrence: RecurrenceType::None,
        recurrence_rule: None,
        expected_relevant: true,
        expected_sensitive_allowed: false,
        duplicate_of: None,
    }
}

/// Generates the deterministic fixture bank for a benchmark configuration.
pub fn generate_fixture_bank(config: &BenchmarkConfig) -> Result<FixtureBank, String> {
    if config.scale_multiplier == 0 {
        return Err("scale_multiplier must be greater than zero".to_string());
    }
    let now = fixed_now();
    let mut facts = vec![
        fact(
            "identity-name",
            "email",
            "has_name",
            "Devansh Rajpal",
            SourceType::UserEdit,
            &[110],
            None,
            None,
        ),
        fact(
            "identity-residence",
            "notes",
            "resides_in",
            "London",
            SourceType::Import,
            &[110],
            None,
            None,
        ),
        literal_fact(
            "preference-food",
            "chat",
            "prefers",
            "vegetarian food",
            SourceType::Interaction,
            &[210],
        ),
        fact(
            "relationship-partner",
            "email",
            "has_partner",
            "Alice",
            SourceType::Interaction,
            &[420],
            None,
            None,
        ),
        fact(
            "calendar-future",
            "calendar",
            "has_event",
            "Future Appointment",
            SourceType::Connector,
            &[930],
            Some(now + Duration::days(3)),
            Some(now + Duration::days(3) + Duration::hours(1)),
        ),
        fact(
            "calendar-recurring",
            "calendar",
            "has_event",
            "Weekly Standup",
            SourceType::Connector,
            &[930],
            Some(now + Duration::days(1)),
            None,
        ),
        fact(
            "calendar-timezone",
            "calendar",
            "has_event",
            "Cross Timezone Meeting",
            SourceType::Connector,
            &[930],
            Some(now + Duration::days(2)),
            Some(now + Duration::days(2) + Duration::hours(1)),
        ),
        fact(
            "calendar-overdue",
            "email",
            "has_event",
            "Overdue Appointment",
            SourceType::Connector,
            &[930],
            Some(now - Duration::days(1)),
            Some(now - Duration::hours(23)),
        ),
        fact(
            "photo-location",
            "photo",
            "took_photo_at",
            "Kyoto",
            SourceType::Connector,
            &[820],
            Some(now - Duration::days(30)),
            None,
        ),
        fact(
            "assistant-state",
            "home-assistant",
            "has_event",
            "Home Comfort Routine",
            SourceType::Connector,
            &[920],
            None,
            None,
        ),
        fact(
            "vision-object",
            "vision-shaped",
            "has_event",
            "Keys Seen In Kitchen",
            SourceType::Connector,
            &[920],
            None,
            None,
        ),
        fact(
            "long-horizon",
            "notes",
            "visited",
            "Rome",
            SourceType::Import,
            &[820],
            Some(now - Duration::days(400)),
            Some(now - Duration::days(398)),
        ),
        literal_fact(
            "sensitive-condition",
            "email",
            "health_condition",
            "diabetes",
            SourceType::Interaction,
            &[320],
        ),
    ];
    let mut duplicate = fact(
        "duplicate-email",
        "email",
        "has_event",
        "Future Appointment",
        SourceType::Interaction,
        &[930],
        Some(now + Duration::days(3)),
        Some(now + Duration::days(3) + Duration::hours(1)),
    );
    duplicate.duplicate_of = Some("calendar-future".to_string());
    duplicate.expected_relevant = false;
    facts.push(duplicate);

    if let Some(fact) = facts
        .iter_mut()
        .find(|fact| fact.id == "calendar-recurring")
    {
        fact.recurrence = RecurrenceType::Weekly;
        fact.recurrence_rule = Some("FREQ=WEEKLY".to_string());
    }

    let mut non_sensitive_flagged = literal_fact(
        "non-sensitive-flagged",
        "chat",
        "prefers",
        "photography books",
        SourceType::Interaction,
        &[770],
    );
    non_sensitive_flagged.is_sensitive = true;
    non_sensitive_flagged.expected_sensitive_allowed = true;
    facts.push(non_sensitive_flagged);

    for index in 0..config.scale_multiplier.max(1) {
        let mut filler = fact(
            "filler",
            "notes",
            "has_event",
            "Filler Event",
            SourceType::Interaction,
            &[960],
            None,
            None,
        );
        filler.object = format!("Filler Event {index}");
        filler.raw_reference = Some(format!("notes:filler-{index}"));
        filler.expected_relevant = false;
        facts.push(filler);
    }

    let queries = vec![
        FixtureQuery {
            id: "memory-ranking".to_string(),
            query: "important upcoming facts".to_string(),
            expected_fact_ids: vec![
                "identity-name".to_string(),
                "identity-residence".to_string(),
                "calendar-future".to_string(),
                "calendar-recurring".to_string(),
            ],
            expected_top_fact_ids: vec![
                "identity-name".to_string(),
                "identity-residence".to_string(),
                "calendar-future".to_string(),
                "calendar-recurring".to_string(),
            ],
        },
        FixtureQuery {
            id: "memory-privacy".to_string(),
            query: "sensitive health context".to_string(),
            expected_fact_ids: vec!["sensitive-condition".to_string()],
            expected_top_fact_ids: Vec::new(),
        },
    ];

    Ok(FixtureBank {
        fixture_version: config.fixture_version,
        facts,
        queries,
    })
}

async fn ingest(
    kg: &crate::KnowledgeGraph,
    bank: &FixtureBank,
) -> Result<HashMap<String, i32>, crate::KnowledgeError> {
    kg.create_entity("Devansh", crate::models::entity::EntityType::Person, &[])
        .await?;
    let mut fact_ids = HashMap::new();
    let mut normalized = Vec::new();
    for item in &bank.facts {
        normalized.push(NormalizedFact {
            confidence: None,
            source_type: item.source_type,
            subject: item.subject.clone(),
            subject_type: EntityType::Person,
            relationship_type: item.predicate.to_string(),
            object: item.object.clone(),
            object_is_entity: item.object_is_entity,
            object_type: item.object_type.map(|kind| match kind {
                "Event" => EntityType::Event,
                "Place" => EntityType::Place,
                "Person" => EntityType::Person,
                _ => EntityType::Object,
            }),
            valid_from: item.valid_from,
            valid_until: item.valid_until,
            is_sensitive: item.is_sensitive,
            is_correction: false,
            correction_scope: None,
            category_ids: item.category_ids.clone(),
            recurrence: item.recurrence,
            recurrence_rule: item.recurrence_rule.clone(),
            recurrence_interval: 1,
            recurrence_until: None,
            requires_user_action: item.requires_user_action,
            raw_reference: item.raw_reference.clone(),
            extraction_method: None,
            event_type: None,
            location: None,
        });
    }
    let provenance = Provenance::chat(ExtractionMethod::UserInput);
    let outcome = normalize_and_insert(kg, normalized, provenance).await?;
    if !outcome.errors.is_empty() {
        return Err(outcome.errors.into_iter().next().unwrap());
    }

    for item in &bank.facts {
        let raw_reference = item.raw_reference.as_ref().ok_or_else(|| {
            crate::KnowledgeError::Validation("fixture has no raw reference".to_string())
        })?;
        let (fact_id,): (i32,) =
            sqlx::query_as("SELECT fact_id FROM sources WHERE raw_reference = ? LIMIT 1")
                .bind(raw_reference)
                .fetch_one(kg.pool())
                .await?;
        fact_ids.insert(item.id.clone(), fact_id);
    }
    Ok(fact_ids)
}

/// Calculates the nearest-rank percentile from microsecond samples.
fn percentile(values: &[u128], percentile: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64 - 1.0) * percentile).round() as usize;
    sorted[index]
}

/// Rounds a performance metric to three decimal places.
fn round_to_three(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

/// Runs the deterministic memory benchmark and returns its JSON report.
pub async fn run_memory_benchmark(
    config: &BenchmarkConfig,
) -> Result<BenchmarkReport, crate::KnowledgeError> {
    let started = std::time::Instant::now();
    let bank = generate_fixture_bank(config).map_err(crate::KnowledgeError::Validation)?;
    let dir = TempDir::new().map_err(crate::KnowledgeError::Io)?;
    let kg = crate::KnowledgeGraph::init_with_clock(
        &dir.path().join("knowledge.db"),
        std::sync::Arc::new(MockClock::new(fixed_now())),
    )
    .await?;
    let ingestion_started = std::time::Instant::now();
    let fact_ids = ingest(&kg, &bank).await?;
    let ingestion_elapsed = ingestion_started.elapsed();
    let subject = kg
        .create_entity("Devansh", crate::models::entity::EntityType::Person, &[])
        .await?
        .id;

    let stability_dir = TempDir::new().map_err(crate::KnowledgeError::Io)?;
    let stability_kg = crate::KnowledgeGraph::init_with_clock(
        &stability_dir.path().join("knowledge.db"),
        std::sync::Arc::new(MockClock::new(fixed_now())),
    )
    .await?;
    let stability_fact_ids = ingest(&stability_kg, &bank).await?;

    let mut latency_samples = Vec::new();
    let mut rendered = String::new();
    let mut last_schema = None;
    for _ in 0..10 {
        rendered.clear();
        let started_retrieval = std::time::Instant::now();
        let schema = kg
            .build_memory_schema_with_opts(
                subject,
                config.memory_budget,
                config.min_confidence,
                crate::queries::memory::BuildMemoryOptions {
                    exclude_from_budget: vec![crate::models::memory::MemoryBucket::Upcoming],
                    exclude_sensitive: true,
                },
            )
            .await?;
        latency_samples.push(started_retrieval.elapsed().as_micros());
        rendered.push_str(&kg.render_memory_schema(&schema));
        last_schema = Some(schema);
    }

    let mut reverse_fact_ids: HashMap<i32, &str> = HashMap::new();
    for (fixture_id, fact_id) in &fact_ids {
        reverse_fact_ids.insert(*fact_id, fixture_id.as_str());
    }
    let mut quality = BenchmarkMetrics::default().quality;
    let returned_ids: Vec<&str> = last_schema
        .as_ref()
        .map(|schema| schema.all_facts())
        .unwrap_or_default()
        .iter()
        .filter_map(|fact| reverse_fact_ids.get(&fact.fact_id).copied())
        .collect();
    let returned_ids_top5: Vec<&str> = returned_ids.iter().take(5).copied().collect();
    let expected_top: Vec<&str> = bank
        .queries
        .iter()
        .flat_map(|query| query.expected_top_fact_ids.iter().map(String::as_str))
        .collect();
    let top_hits = expected_top
        .iter()
        .filter(|expected| returned_ids.contains(expected))
        .count();
    quality.insert(
        MetricName::RecallAt5.as_str().to_string(),
        if expected_top.is_empty() {
            1.0
        } else {
            top_hits as f64 / expected_top.len() as f64
        },
    );
    quality.insert(
        MetricName::PrecisionAt5.as_str().to_string(),
        if returned_ids.is_empty() {
            0.0
        } else {
            top_hits as f64 / returned_ids_top5.len().max(1) as f64
        },
    );

    let mut provenance_matches = 0usize;
    let mut citation_matches = 0usize;
    let mut temporal_matches = 0usize;
    let mut checked_returned = 0usize;
    for fixture_id in &returned_ids_top5 {
        let Some(fixture_fact) = bank
            .facts
            .iter()
            .find(|candidate| candidate.id == *fixture_id)
        else {
            continue;
        };
        let Some(fact_id) = fact_ids.get(*fixture_id) else {
            continue;
        };
        let sources = kg.get_sources_for_fact(*fact_id).await?;
        let Some(source) = sources.first() else {
            continue;
        };
        checked_returned += 1;
        if source.source_type_id == fixture_fact.source_type as i16 {
            provenance_matches += 1;
        }
        if source.raw_reference == fixture_fact.raw_reference {
            citation_matches += 1;
        }
        if let Some(fact) = kg.get_fact(*fact_id).await? {
            if fact.valid_from == fixture_fact.valid_from
                && fact.valid_until == fixture_fact.valid_until
            {
                temporal_matches += 1;
            }
        }
    }
    let denominator = checked_returned.max(1) as f64;
    quality.insert(
        MetricName::ProvenanceAccuracy.as_str().to_string(),
        provenance_matches as f64 / denominator,
    );
    quality.insert(
        MetricName::CitationFabricationRate.as_str().to_string(),
        1.0 - (citation_matches as f64 / denominator),
    );
    quality.insert(
        MetricName::TemporalCorrectness.as_str().to_string(),
        temporal_matches as f64 / denominator,
    );

    let stable_pair_count = fact_ids
        .iter()
        .filter(|(fixture_id, fact_id)| stability_fact_ids.get(*fixture_id) == Some(*fact_id))
        .count();
    quality.insert(
        MetricName::ConsolidationStability.as_str().to_string(),
        stable_pair_count as f64 / fact_ids.len().max(1) as f64,
    );

    let mut dedup_correct = 0usize;
    let mut dedup_checked = 0usize;
    for duplicate in bank.facts.iter().filter_map(|candidate| {
        candidate
            .duplicate_of
            .as_ref()
            .map(|original| (candidate, original))
    }) {
        dedup_checked += 1;
        if fact_ids.get(&duplicate.0.id) == fact_ids.get(duplicate.1) {
            dedup_correct += 1;
        }
    }
    quality.insert(
        MetricName::DedupPrecision.as_str().to_string(),
        dedup_correct as f64 / dedup_checked.max(1) as f64,
    );

    let mut sensitive_allowed = 0usize;
    let mut sensitive_checked = 0usize;
    let mut non_sensitive_blocked = 0usize;
    let mut non_sensitive_checked = 0usize;
    for item in &bank.facts {
        let Some(fact_id) = fact_ids.get(&item.id) else {
            continue;
        };
        let (pending,): (bool,) =
            sqlx::query_as("SELECT pending_confirmation FROM facts WHERE id = ?")
                .bind(fact_id)
                .fetch_one(kg.pool())
                .await?;
        if item.is_sensitive && !item.expected_sensitive_allowed {
            sensitive_checked += 1;
            if !pending {
                sensitive_allowed += 1;
            }
        } else if item.is_sensitive && item.expected_sensitive_allowed {
            non_sensitive_checked += 1;
            if pending {
                non_sensitive_blocked += 1;
            }
        }
    }
    quality.insert(
        MetricName::PrivacyFalseAllowRate.as_str().to_string(),
        sensitive_allowed as f64 / sensitive_checked.max(1) as f64,
    );
    quality.insert(
        MetricName::PrivacyFalseBlockRate.as_str().to_string(),
        non_sensitive_blocked as f64 / non_sensitive_checked.max(1) as f64,
    );

    let mut performance = BenchmarkMetrics::default().performance;
    performance.insert(
        PerformanceName::RetrievalLatencyP95.as_str().to_string(),
        percentile(&latency_samples, 0.95) as f64,
    );
    performance.insert(
        PerformanceName::RetrievalLatencyP99.as_str().to_string(),
        percentile(&latency_samples, 0.99) as f64,
    );
    performance.insert(
        PerformanceName::IngestionThroughput.as_str().to_string(),
        round_to_three(bank.facts.len() as f64 / ingestion_elapsed.as_secs_f64().max(0.000001)),
    );
    let db_bytes = tokio::fs::metadata(dir.path().join("knowledge.db"))
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    performance.insert(
        PerformanceName::MemoryIndexSize.as_str().to_string(),
        db_bytes as f64,
    );
    performance.insert(
        PerformanceName::RenderedTokenOutput.as_str().to_string(),
        (rendered.len() as f64 / 4.0).ceil(),
    );
    performance.insert(
        PerformanceName::BenchmarkWallTime.as_str().to_string(),
        started.elapsed().as_millis() as f64,
    );

    let mut violations = Vec::new();
    for (metric, threshold) in &config.thresholds {
        let value = quality.get(metric.as_str()).copied().unwrap_or(0.0);
        let kind = if metric.is_lower_better() {
            ViolationKind::AboveMaximum
        } else {
            ViolationKind::BelowMinimum
        };
        let violated = match kind {
            ViolationKind::BelowMinimum => value < *threshold,
            ViolationKind::AboveMaximum => value > *threshold,
        };
        if violated {
            violations.push(Violation {
                metric: ViolationMetric::Quality(*metric),
                value,
                threshold: *threshold,
                kind,
            });
        }
    }
    for (metric, budget) in &config.performance_budgets {
        let value = performance.get(metric.as_str()).copied().unwrap_or(0.0);
        let kind = if metric.is_higher_better() {
            ViolationKind::BelowMinimum
        } else {
            ViolationKind::AboveMaximum
        };
        let violated = match kind {
            ViolationKind::BelowMinimum => value < *budget,
            ViolationKind::AboveMaximum => value > *budget,
        };
        if violated {
            violations.push(Violation {
                metric: ViolationMetric::Performance(*metric),
                value,
                threshold: *budget,
                kind,
            });
        }
    }

    let metrics = BenchmarkMetrics {
        quality,
        performance,
    };

    let report = BenchmarkReport {
        schema_version: 1,
        fixture_version: config.fixture_version,
        generated_at: chrono::Utc::now().to_rfc3339(),
        metrics,
        violations,
    };
    Ok(report)
}

/// Writes a benchmark report to the requested baseline file.
pub async fn save_baseline(
    report: &BenchmarkReport,
    path: &std::path::Path,
) -> Result<(), crate::KnowledgeError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let json = serde_json::to_vec_pretty(report).map_err(|error| {
        crate::KnowledgeError::Validation(format!("baseline serialization failed: {error}"))
    })?;
    tokio::fs::write(path, json).await?;
    Ok(())
}

/// Reads and validates a benchmark report baseline file.
pub async fn load_baseline(
    path: &std::path::Path,
) -> Result<BenchmarkReport, crate::KnowledgeError> {
    let bytes = tokio::fs::read(path).await?;
    let report: BenchmarkReport = serde_json::from_slice(&bytes).map_err(|error| {
        crate::KnowledgeError::Validation(format!("baseline deserialization failed: {error}"))
    })?;
    if report.schema_version != 1 {
        return Err(crate::KnowledgeError::Validation(format!(
            "unsupported baseline schema version: {}",
            report.schema_version
        )));
    }
    if report.fixture_version == 0 {
        return Err(crate::KnowledgeError::Validation(
            "baseline fixture version must be greater than zero".to_string(),
        ));
    }
    for metric in QUALITY_METRICS {
        match report.metrics.quality.get(metric.as_str()).copied() {
            Some(value) if value.is_finite() => {}
            value => {
                return Err(crate::KnowledgeError::Validation(format!(
                    "missing baseline metric: {}",
                    value.map(|_| metric.as_str()).unwrap_or(metric.as_str())
                )));
            }
        }
    }
    for metric in PERFORMANCE_METRICS {
        match report.metrics.performance.get(metric.as_str()).copied() {
            Some(value) if value.is_finite() => {}
            _ => {
                return Err(crate::KnowledgeError::Validation(format!(
                    "missing baseline metric: {}",
                    metric.as_str()
                )));
            }
        }
    }
    Ok(report)
}
