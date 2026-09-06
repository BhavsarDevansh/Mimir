//! Write-path and setup benchmarks for the knowledge graph.
//!
//! These mirror the costs the integration tests pay repeatedly:
//!
//! - fresh-DB initialisation (58 migrations) — every test that calls
//!   `KnowledgeGraph::init` pays this once;
//! - single-fact insertion — the temporal-overlap scan, memory-priority
//!   lookup, source row, and audit row per insert;
//! - same-subject insert growth — the overlap scan re-reads every existing
//!   fact for the subject/predicate pair;
//! - entity creation with aliases — per-alias `INSERT OR IGNORE` loop;
//! - the nightly dedup pass — the O(n^2) self-join over same-subject facts;
//! - BFS traversal of a star graph — the `node_cap` traversal path.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::optimization::{OptimizationConfig, OptimizationRunner, PassName};
use mimir_knowledge::queries;

fn new_fact(
    subject_id: i32,
    predicate: &str,
    object_id: Option<i32>,
    object_literal: Option<&str>,
    raw_reference: &str,
) -> NewFact {
    NewFact {
        subject_id,
        relationship_type: predicate.to_string(),
        object_id,
        object_literal: object_literal.map(str::to_string),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::Connector,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: Some(raw_reference.to_string()),
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    }
}

/// Fresh graph + `entity_count` entities, ready for insert measurement.
fn seeded_graph(
    rt: &tokio::runtime::Runtime,
    entity_count: usize,
) -> (KnowledgeGraph, tempfile::TempDir, Vec<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let kg = rt
        .block_on(KnowledgeGraph::init(&dir.path().join("kg.db")))
        .unwrap();
    let mut ids = Vec::with_capacity(entity_count);
    for i in 0..entity_count {
        let ty = if i % 2 == 0 {
            EntityType::Person
        } else {
            EntityType::Place
        };
        let e = rt
            .block_on(kg.create_entity(&format!("Entity{i}"), ty, &[]))
            .unwrap();
        ids.push(e.id);
    }
    (kg, dir, ids)
}

fn bench_schema_init(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_schema_init", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                rt.block_on(async {
                    let kg = KnowledgeGraph::init(&dir.path().join("kg.db"))
                        .await
                        .unwrap();
                    std::hint::black_box(kg.pool());
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_schema_init_from_template(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_schema_init_from_template", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                rt.block_on(async {
                    let db_path = dir.path().join("kg.db");
                    mimir_test_support::prepare_from_template(&db_path)
                        .await
                        .unwrap();
                    let kg = KnowledgeGraph::init(&db_path).await.unwrap();
                    std::hint::black_box(kg.pool());
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_fact_insert_small_graph(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_fact_insert_small_graph", |b| {
        b.iter_batched(
            || seeded_graph(&rt, 6),
            |(kg, _dir, ids)| {
                rt.block_on(async {
                    let mut last: Option<i32> = None;
                    for i in 0..10 {
                        // Persons live at even indices, places at odd
                        // indices; "visited" requires Person -> Place.
                        let subject = ids[(i % 3) * 2];
                        let object = Some(ids[(i % 3) * 2 + 1]);
                        let fact = new_fact(subject, "visited", object, None, &format!("raw-{i}"));
                        last = Some(kg.insert_fact(fact).await.unwrap().id);
                    }
                    std::hint::black_box(last);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_fact_insert_same_subject_growth(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_fact_insert_same_subject_growth", |b| {
        b.iter_batched(
            || {
                let (kg, dir, ids) = seeded_graph(&rt, 62);
                rt.block_on(async {
                    // 30 pre-existing facts on subject 0 (the overlap scan
                    // re-reads all of them on every insert).
                    for i in 0..30 {
                        let fact = new_fact(
                            ids[0],
                            "visited",
                            Some(ids[2 * i + 1]),
                            None,
                            &format!("pre-{i}"),
                        );
                        kg.insert_fact(fact).await.unwrap();
                    }
                });
                (kg, dir, ids)
            },
            |(kg, _dir, ids)| {
                rt.block_on(async {
                    let fact = new_fact(ids[0], "visited", Some(ids[61]), None, "measured");
                    std::hint::black_box(kg.insert_fact(fact).await.unwrap().id);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_entity_create_with_aliases(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_entity_create_with_aliases", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                rt.block_on(async {
                    let kg = KnowledgeGraph::init(&dir.path().join("kg.db"))
                        .await
                        .unwrap();
                    (kg, dir)
                })
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    for i in 0..5 {
                        let aliases = ["one", "two", "three"];
                        let e = kg
                            .create_entity(&format!("Person {i}"), EntityType::Person, &aliases)
                            .await
                            .unwrap();
                        std::hint::black_box(e.id);
                    }
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_dedup_pass(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_optimization_dedup_pass_100", |b| {
        b.iter_batched(
            || {
                // Seed 100 facts (50 distinct triples x2 duplicates) directly
                // so the measured pass is not gated on insert-pipeline cost.
                let (kg, dir, ids) = seeded_graph(&rt, 30);
                rt.block_on(async {
                    let knows = mimir_test_support::ensure_relationship_type(&kg, "knows")
                        .await
                        .unwrap();
                    let normal: i16 = sqlx::query_scalar(
                        "SELECT id FROM memory_priorities WHERE name = 'Normal'",
                    )
                    .fetch_one(kg.pool())
                    .await
                    .unwrap();
                    let now = chrono::Utc::now();
                    for i in 0..50 {
                        let subject = ids[i % 5];
                        let object = ids[5 + (i % 5)];
                        for _dup in 0..2 {
                            sqlx::query(
                                "INSERT INTO facts \
                                 (subject_id, relationship_type_id, object_id, object_literal, \
                                  valid_from, valid_until, confidence, fact_status_id, inferred, \
                                  inference_depth, pending_confirmation, memory_priority_id, \
                                  created_at, updated_at) \
                                 VALUES (?, ?, ?, NULL, NULL, NULL, 0.8, ?, 0, 0, 0, ?, ?, ?)",
                            )
                            .bind(subject)
                            .bind(knows)
                            .bind(object)
                            .bind(FactStatus::Active as i16)
                            .bind(normal)
                            .bind(now)
                            .bind(now)
                            .execute(kg.pool())
                            .await
                            .unwrap();
                        }
                    }
                });
                (kg, dir)
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    let runner = OptimizationRunner::new(
                        &kg,
                        OptimizationConfig::for_test(std::path::PathBuf::from("/tmp/bench")),
                        None,
                    );
                    let summary = runner.run_pass(PassName::Deduplication).await.unwrap();
                    std::hint::black_box(summary.facts_merged);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_traverse_star_graph(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("kg_traverse_star_300_node_cap_200", |b| {
        b.iter_batched(
            || {
                let (kg, dir, ids) = seeded_graph(&rt, 300);
                rt.block_on(async {
                    let knows = mimir_test_support::ensure_relationship_type(&kg, "knows")
                        .await
                        .unwrap();
                    let normal: i16 = sqlx::query_scalar(
                        "SELECT id FROM memory_priorities WHERE name = 'Normal'",
                    )
                    .fetch_one(kg.pool())
                    .await
                    .unwrap();
                    let now = chrono::Utc::now();
                    for i in 1..300 {
                        sqlx::query(
                            "INSERT INTO facts \
                             (subject_id, relationship_type_id, object_id, object_literal, \
                              confidence, fact_status_id, inferred, inference_depth, \
                              pending_confirmation, memory_priority_id, created_at, updated_at) \
                             VALUES (?, ?, ?, NULL, 0.9, ?, 0, 0, 0, ?, ?, ?)",
                        )
                        .bind(ids[0])
                        .bind(knows)
                        .bind(ids[i])
                        .bind(FactStatus::Active as i16)
                        .bind(normal)
                        .bind(now)
                        .bind(now)
                        .execute(kg.pool())
                        .await
                        .unwrap();
                    }
                });
                (kg, dir)
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    let result = queries::traverse::traverse_graph(kg.pool(), 1, 1, 200, None)
                        .await
                        .unwrap();
                    std::hint::black_box(result.edges.len());
                });
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    kg_write_benches,
    bench_schema_init,
    bench_schema_init_from_template,
    bench_fact_insert_small_graph,
    bench_fact_insert_same_subject_growth,
    bench_entity_create_with_aliases,
    bench_dedup_pass,
    bench_traverse_star_graph
);
criterion_main!(kg_write_benches);
