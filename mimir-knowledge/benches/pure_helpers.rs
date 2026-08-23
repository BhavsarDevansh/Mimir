//! Micro-benchmarks for non-hotpath pure helpers in mimir-knowledge.
//!
//! Covers confidence scoring, memory priority boosts, retrieval summary
//! generation, fact identity comparison, and recurrence-date arithmetic.

use chrono::{TimeZone, Utc};
use criterion::{Criterion, criterion_group, criterion_main};
use mimir_knowledge::{
    confidence, geo,
    models::{
        enums::{ConnectorType, RecurrenceType},
        memory::{MemoryBucket, MemoryPriority, MemorySchema, RankedFact},
        recurrence::next_occurrence,
        source::SourceType,
    },
    retrieval::{RetrievedContext, RetrievedEntity, RetrievedFact, RetrievedRelation},
};
use std::hint::black_box;

fn bench_confidence_initial(c: &mut Criterion) {
    let combos = [
        (SourceType::UserEdit, None),
        (SourceType::System, None),
        (SourceType::Interaction, None),
        (SourceType::Import, None),
        (SourceType::Inference, None),
        (SourceType::Connector, Some(ConnectorType::Calendar)),
        (SourceType::Connector, Some(ConnectorType::Email)),
        (SourceType::Connector, None),
    ];
    c.bench_function("confidence_initial", |b| {
        b.iter(|| {
            for (st, ct) in combos {
                black_box(confidence::initial(st, ct));
            }
        })
    });
}

fn bench_confidence_inference(c: &mut Criterion) {
    let parents: Vec<(f32, bool)> = (0..20)
        .map(|i| (1.0 / (i as f32 + 1.0), i % 2 == 0))
        .collect();
    c.bench_function("confidence_inference_20_parents", |b| {
        b.iter(|| black_box(confidence::inference_confidence(&parents, 3, parents.len())))
    });
    let small = [(0.9, true), (0.8, false), (0.7, true)];
    c.bench_function("confidence_inference_3_parents", |b| {
        b.iter(|| black_box(confidence::inference_confidence(&small, 1, 3)))
    });
}

fn bench_confidence_default_connector(c: &mut Criterion) {
    let connectors = [
        ConnectorType::Email,
        ConnectorType::Calendar,
        ConnectorType::Photos,
        ConnectorType::LinkedIn,
    ];
    c.bench_function("confidence_default_connector_score", |b| {
        b.iter(|| {
            for ct in connectors {
                black_box(confidence::default_connector_score(ct));
            }
        })
    });
}

fn bench_memory_priority_boost(c: &mut Criterion) {
    let tiers = [
        MemoryPriority::Critical,
        MemoryPriority::High,
        MemoryPriority::Normal,
        MemoryPriority::Low,
    ];
    c.bench_function("memory_priority_boost", |b| {
        b.iter(|| {
            for p in tiers {
                black_box(p.boost());
            }
        })
    });
}

fn bench_retrieval_summary(c: &mut Criterion) {
    let ctx = RetrievedContext {
        entities: (0..10)
            .map(|i| RetrievedEntity {
                name: format!("entity{i}"),
                entity_type: "person".to_string(),
                facts: (0..5)
                    .map(|j| RetrievedFact {
                        predicate: format!("pred{j}"),
                        object_name: Some(format!("obj{j}")),
                        object_literal: None,
                        confidence: 0.8,
                        valid_from: None,
                        valid_until: None,
                        status: "active".to_string(),
                        inferred: false,
                    })
                    .collect(),
            })
            .collect(),
        relations: (0..20)
            .map(|i| RetrievedRelation {
                subject_name: format!("s{i}"),
                predicate: "knows".to_string(),
                object_name: format!("o{i}"),
                depth: 1,
            })
            .collect(),
        conversation_snippets: vec![],
        finish_reason: None,
        rounds_used: 4,
    };
    c.bench_function("retrieval_context_summary", |b| {
        b.iter(|| black_box(ctx.summary()))
    });
}

fn bench_retrieval_fact_same_identity(c: &mut Criterion) {
    let fact = RetrievedFact {
        predicate: "lives_in".to_string(),
        object_name: Some("London".to_string()),
        object_literal: None,
        confidence: 0.9,
        valid_from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
        valid_until: None,
        status: "active".to_string(),
        inferred: false,
    };
    let other = fact.clone();
    c.bench_function("retrieval_fact_same_identity", |b| {
        b.iter(|| black_box(fact.same_identity(&other)))
    });
}

fn bench_next_occurrence(c: &mut Criterion) {
    let from = Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap();
    let cases = [
        ("1990-06-15", RecurrenceType::Yearly),
        ("2024-03-01", RecurrenceType::Monthly),
        ("2024-06-14", RecurrenceType::Weekly),
        ("2024-06-14", RecurrenceType::Daily),
        ("2024-02-29", RecurrenceType::Yearly),
    ];
    c.bench_function("next_occurrence_mixed", |b| {
        b.iter(|| {
            for (date, rec) in cases {
                black_box(next_occurrence(date, rec, from));
            }
        })
    });
}

fn bench_memory_schema_all_facts(c: &mut Criterion) {
    let mk = |id: i32, bucket: MemoryBucket| RankedFact {
        fact_id: id,
        subject_name: format!("s{id}"),
        relationship_type: "r".to_string(),
        object_display: "o".to_string(),
        confidence: 1.0,
        score: 1.0,
        temporal_boost: 0.0,
        memory_weight: 1.0,
        priority_boost: 1.0,
        centrality_boost: 0.0,
        category_ids: vec![],
        bucket,
        char_estimate: 10,
    };
    let schema = MemorySchema {
        identity: (0..5).map(|i| mk(i, MemoryBucket::Identity)).collect(),
        relationships: (5..15)
            .map(|i| mk(i, MemoryBucket::Relationships))
            .collect(),
        preferences: (15..20).map(|i| mk(i, MemoryBucket::Preferences)).collect(),
        upcoming: vec![],
        general: (20..30).map(|i| mk(i, MemoryBucket::General)).collect(),
        total_score: 30.0,
        char_count: 300,
    };
    c.bench_function("memory_schema_all_facts", |b| {
        b.iter(|| black_box(schema.all_facts()))
    });
}

fn bench_haversine(c: &mut Criterion) {
    // London <-> Paris (~346 km) and a short hop (~1.3 km): cover the long- and
    // short-distance paths of the Haversine inner loop.
    c.bench_function("haversine_km", |b| {
        b.iter(|| {
            black_box(geo::haversine_km(51.5074, -0.1278, 48.8566, 2.3522));
            black_box(geo::haversine_km(51.5074, -0.1278, 51.5190, -0.1278));
        })
    });
}

criterion_group!(
    pure_helpers,
    bench_confidence_initial,
    bench_confidence_inference,
    bench_confidence_default_connector,
    bench_memory_priority_boost,
    bench_retrieval_summary,
    bench_retrieval_fact_same_identity,
    bench_next_occurrence,
    bench_memory_schema_all_facts,
    bench_haversine,
);
criterion_main!(pure_helpers);
