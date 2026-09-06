//! Query unit tests for `kg_related` traversal logic.

use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::queries::traverse::traverse_graph;

mod common;

#[tokio::test]
async fn test_kg_traverse_linear_chain() {
    let tg = common::TestGraph::new().await;
    let a = tg.create_person("A").await;
    let b = tg.create_person("B").await;
    let c = tg.create_person("C").await;

    let f1 = NewFact {
        subject_id: a,
        relationship_type: "has_partner".to_string(),
        object_id: Some(b),
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
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    let f2 = NewFact {
        subject_id: b,
        relationship_type: "has_partner".to_string(),
        object_id: Some(c),
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
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f1).await.unwrap();
    tg.kg.insert_fact(f2).await.unwrap();

    let result = traverse_graph(tg.kg.pool(), a as u32, 2, 50, None)
        .await
        .unwrap();

    assert!(!result.edges.is_empty());
    let depths: Vec<u32> = result.edges.iter().map(|e| e.depth).collect();
    assert!(depths.contains(&0));
    assert!(depths.contains(&1));
}

#[tokio::test]
async fn test_kg_traverse_cycle() {
    let tg = common::TestGraph::new().await;
    let a = tg.create_person("A").await;
    let b = tg.create_person("B").await;

    let f1 = NewFact {
        subject_id: a,
        relationship_type: "has_partner".to_string(),
        object_id: Some(b),
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
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    let f2 = NewFact {
        subject_id: b,
        relationship_type: "has_partner".to_string(),
        object_id: Some(a),
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
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f1).await.unwrap();
    tg.kg.insert_fact(f2).await.unwrap();

    let result = traverse_graph(tg.kg.pool(), a as u32, 3, 50, None)
        .await
        .unwrap();

    // B should appear only once (at depth 0).
    let b_edges: Vec<_> = result.edges.iter().filter(|e| e.object == "B").collect();
    assert_eq!(b_edges.len(), 1);
}

#[tokio::test]
async fn test_kg_traverse_depth_cap() {
    let tg = common::TestGraph::new().await;
    let a = tg.create_person("A").await;
    let b = tg.create_person("B").await;

    let f1 = NewFact {
        subject_id: a,
        relationship_type: "has_partner".to_string(),
        object_id: Some(b),
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
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f1).await.unwrap();

    // Request depth 10, but tool clamps to 5. Test traversal directly with 5.
    let result = traverse_graph(tg.kg.pool(), a as u32, 5, 50, None)
        .await
        .unwrap();
    assert_eq!(result.max_depth_reached, 1);
}

#[tokio::test]
async fn test_kg_traverse_node_cap() {
    let tg = common::TestGraph::new().await;
    let root = tg.create_person("Root").await;

    // Star graph: Root -> Node_1, Node_2, ..., Node_300
    for i in 0..300 {
        let node = tg
            .kg
            .create_entity(&format!("Node_{}", i), EntityType::Person, &[])
            .await
            .unwrap();
        let f = NewFact {
            subject_id: root,
            relationship_type: "has_partner".to_string(),
            object_id: Some(node.id),
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
            confidence: Some(0.9),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        };
        tg.kg.insert_fact(f).await.unwrap();
    }

    let result = traverse_graph(tg.kg.pool(), root as u32, 1, 200, None)
        .await
        .unwrap();

    assert!(
        result.nodes_found <= 200,
        "expected nodes_found <= 200, got {}",
        result.nodes_found
    );
}

#[tokio::test]
async fn test_kg_traverse_predicate_filter() {
    let tg = common::TestGraph::new().await;
    let a = tg.create_person("A").await;
    let b = tg.create_person("B").await;
    let c = tg.create_place("C").await;

    let f1 = NewFact {
        subject_id: a,
        relationship_type: "has_partner".to_string(),
        object_id: Some(b),
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
        confidence: Some(0.9),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    let f2 = NewFact {
        subject_id: a,
        relationship_type: "visited".to_string(),
        object_id: Some(c),
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
        confidence: Some(0.8),
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    };
    tg.kg.insert_fact(f1).await.unwrap();
    tg.kg.insert_fact(f2).await.unwrap();

    let pred_id = common::ensure_relationship_type(&tg.kg, "has_partner")
        .await
        .unwrap();
    let result = traverse_graph(tg.kg.pool(), a as u32, 1, 50, Some(&[pred_id]))
        .await
        .unwrap();

    assert_eq!(result.edges.len(), 1);
    assert_eq!(result.edges[0].predicate, "has_partner");
}
