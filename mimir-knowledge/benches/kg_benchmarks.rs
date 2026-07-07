use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries;

/// Seed ~10 000 facts into a fresh knowledge graph.
async fn seed_10k_facts(kg: &KnowledgeGraph) -> (Vec<i32>, Vec<i32>) {
    let _is_in_id = kg.ensure_relationship_type("is_in").await.unwrap();
    let _visited_id = kg.ensure_relationship_type("visited").await.unwrap();
    let _knows_id = kg.ensure_relationship_type("knows").await.unwrap();

    // Create 500 persons and 500 places (1000 entities total)
    let mut person_ids = Vec::with_capacity(500);
    let mut place_ids = Vec::with_capacity(500);

    for i in 0..500 {
        let p = kg
            .create_entity(&format!("Person{}", i), EntityType::Person, &[])
            .await
            .unwrap();
        person_ids.push(p.id);
    }
    for i in 0..500 {
        let p = kg
            .create_entity(&format!("Place{}", i), EntityType::Place, &[])
            .await
            .unwrap();
        place_ids.push(p.id);
    }

    // Add aliases to ~10% of entities for FTS5 indexing
    for (i, &pid) in person_ids.iter().enumerate().take(50) {
        kg.add_alias(pid, &format!("Alias{}", i)).await.unwrap();
    }

    // Seed 10k facts: is_in, visited, knows
    for i in 0..10_000 {
        let subject = person_ids[i % person_ids.len()];
        let pred = match i % 3 {
            0 => "is_in",
            1 => "visited",
            _ => "knows",
        };
        let object = match i % 3 {
            0 | 1 => Some(place_ids[i % place_ids.len()]),
            _ => Some(person_ids[(i + 1) % person_ids.len()]),
        };
        let nf = NewFact {
            subject_id: subject,
            relationship_type: pred.to_string(),
            object_id: object,
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        };
        kg.insert_fact(nf).await.unwrap();
    }
    (person_ids, place_ids)
}

fn bench_entity_resolution_exact(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("entity_resolution_exact", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("kg.db");
                let kg = rt.block_on(KnowledgeGraph::init(&db)).unwrap();
                rt.block_on(seed_10k_facts(&kg));
                (kg, dir)
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    let e = queries::entity::get_by_name(kg.pool(), "Person42")
                        .await
                        .unwrap();
                    std::hint::black_box(e);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_entity_resolution_alias(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("entity_resolution_alias", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("kg.db");
                let kg = rt.block_on(KnowledgeGraph::init(&db)).unwrap();
                rt.block_on(seed_10k_facts(&kg));
                (kg, dir)
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    let e = queries::entity::get_by_name(kg.pool(), "Alias10")
                        .await
                        .unwrap();
                    std::hint::black_box(e);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_fts5_search(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("fts5_search", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("kg.db");
                let kg = rt.block_on(KnowledgeGraph::init(&db)).unwrap();
                rt.block_on(seed_10k_facts(&kg));
                (kg, dir)
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    let results = kg.search_entities("Person", 10).await.unwrap();
                    std::hint::black_box(results);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_facts_by_subject_with_chain(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("facts_by_subject_with_chain", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("kg.db");
                let kg = rt.block_on(KnowledgeGraph::init(&db)).unwrap();
                let (person_ids, _place_ids) = rt.block_on(seed_10k_facts(&kg));
                // Build a linear chain using actual seeded person IDs
                for i in 0..3 {
                    let nf = NewFact {
                        subject_id: person_ids[i],
                        relationship_type: "knows".to_string(),
                        object_id: Some(person_ids[i + 1]),
                        object_literal: None,
                        valid_from: None,
                        valid_until: None,
                        source_type: SourceType::UserEdit,
                        connector_instance_id: None,
                        connector_type: None,
                        raw_reference: None,
                        extraction_method: None,
                        inferred: false,
                        inference_depth: 0,
                        confidence: None,
                        parent_fact_ids: Vec::new(),
                        category_ids: Vec::new(),
                    };
                    rt.block_on(kg.insert_fact(nf)).unwrap();
                }
                (kg, dir, person_ids)
            },
            |(kg, _dir, person_ids)| {
                rt.block_on(async {
                    let results = kg.get_facts_by_subject(person_ids[0], 100).await.unwrap();
                    std::hint::black_box(results);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_inference_chain_100(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("inference_chain_100", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("kg.db");
                let kg = rt.block_on(KnowledgeGraph::init(&db)).unwrap();
                // Build a chain of 100 is_in facts: Place0 is_in Place1 is_in ... Place100
                let mut prev_id = rt
                    .block_on(kg.create_entity("Place0", EntityType::Place, &[]))
                    .unwrap()
                    .id;
                for i in 1..=100 {
                    let next = rt
                        .block_on(kg.create_entity(&format!("Place{}", i), EntityType::Place, &[]))
                        .unwrap();
                    let nf = NewFact {
                        subject_id: prev_id,
                        relationship_type: "is_in".to_string(),
                        object_id: Some(next.id),
                        object_literal: None,
                        valid_from: None,
                        valid_until: None,
                        source_type: SourceType::UserEdit,
                        connector_instance_id: None,
                        connector_type: None,
                        raw_reference: None,
                        extraction_method: None,
                        inferred: false,
                        inference_depth: 0,
                        confidence: None,
                        parent_fact_ids: Vec::new(),
                        category_ids: Vec::new(),
                    };
                    rt.block_on(kg.insert_fact(nf)).unwrap();
                    prev_id = next.id;
                }
                (kg, dir)
            },
            |(kg, _dir)| {
                rt.block_on(async {
                    // Trigger re-evaluation by inserting a new visited fact at Place0
                    let place0_results = queries::entity::get_by_name(kg.pool(), "Place0")
                        .await
                        .unwrap();
                    let place0 = &place0_results[0].entity;
                    let visitor = kg
                        .create_entity("Visitor", EntityType::Person, &[])
                        .await
                        .unwrap();
                    let nf = NewFact {
                        subject_id: visitor.id,
                        relationship_type: "visited".to_string(),
                        object_id: Some(place0.id),
                        object_literal: None,
                        valid_from: None,
                        valid_until: None,
                        source_type: SourceType::UserEdit,
                        connector_instance_id: None,
                        connector_type: None,
                        raw_reference: None,
                        extraction_method: None,
                        inferred: false,
                        inference_depth: 0,
                        confidence: None,
                        parent_fact_ids: Vec::new(),
                        category_ids: Vec::new(),
                    };
                    kg.insert_fact(nf).await.unwrap();
                    let facts = kg.get_facts_by_subject(visitor.id, 200).await.unwrap();
                    std::hint::black_box(facts);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_memory_condensation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("memory_condensation", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("kg.db");
                let kg = rt.block_on(KnowledgeGraph::init(&db)).unwrap();
                let (person_ids, _) = rt.block_on(seed_10k_facts(&kg));
                (kg, dir, person_ids)
            },
            |(kg, _dir, person_ids)| {
                rt.block_on(async {
                    let mem = kg
                        .build_memory_schema(person_ids[0], 2500, 0.5)
                        .await
                        .unwrap();
                    let text = kg.render_memory_schema(&mem);
                    std::hint::black_box(text);
                });
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_entity_resolution_exact,
    bench_entity_resolution_alias,
    bench_fts5_search,
    bench_facts_by_subject_with_chain,
    bench_inference_chain_100,
    bench_memory_condensation
);
criterion_main!(benches);
