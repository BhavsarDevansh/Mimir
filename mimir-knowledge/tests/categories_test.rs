//! Category CRUD tests: memory bucket validation on insert.

use mimir_knowledge::KnowledgeError;
use mimir_knowledge::models::category::NewCategory;
use mimir_knowledge::models::memory::MemoryBucket;

mod common;

#[tokio::test]
async fn insert_category_rejects_unknown_memory_bucket_id() {
    let graph = common::TestGraph::new().await;
    let category = NewCategory {
        id: 99_001,
        name: "Reviewers".to_string(),
        description: None,
        parent_id: None,
        memory_weight: None,
        memory_bucket_id: Some(42),
    };
    let err = graph.kg.insert_category(category).await.unwrap_err();
    match err {
        KnowledgeError::Validation(message) => {
            assert!(message.contains("42"), "message: {message}")
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn insert_category_accepts_seeded_memory_bucket_id() {
    let graph = common::TestGraph::new().await;
    let category = NewCategory {
        id: 99_002,
        name: "Reviewers".to_string(),
        description: None,
        parent_id: None,
        memory_weight: None,
        memory_bucket_id: Some(MemoryBucket::Identity as i16),
    };
    let inserted = graph.kg.insert_category(category).await.unwrap();
    assert_eq!(
        inserted.memory_bucket_id,
        Some(MemoryBucket::Identity as i16)
    );
}
