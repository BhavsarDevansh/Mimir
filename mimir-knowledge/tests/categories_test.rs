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

#[tokio::test]
async fn insert_category_canonicalises_surrounding_whitespace() {
    let graph = common::TestGraph::new().await;
    let category = NewCategory {
        id: 99_003,
        name: "  Reviewers  ".to_string(),
        description: None,
        parent_id: None,
        memory_weight: None,
        memory_bucket_id: None,
    };
    let inserted = graph.kg.insert_category(category).await.unwrap();
    assert_eq!(inserted.name, "Reviewers");
}

#[tokio::test]
async fn insert_category_rejects_whitespace_only_name() {
    let graph = common::TestGraph::new().await;
    let category = NewCategory {
        id: 99_004,
        name: "   ".to_string(),
        description: None,
        parent_id: None,
        memory_weight: None,
        memory_bucket_id: None,
    };
    let result = graph.kg.insert_category(category).await;
    assert!(matches!(result, Err(KnowledgeError::Validation(_))));
}

#[tokio::test]
async fn insert_category_rejects_line_breaks_and_control_characters() {
    let graph = common::TestGraph::new().await;
    for name in ["Bad\nName", "Bad\rName", "Bad\u{7}Name"] {
        let category = NewCategory {
            id: 99_005,
            name: name.to_string(),
            description: None,
            parent_id: None,
            memory_weight: None,
            memory_bucket_id: None,
        };
        let result = graph.kg.insert_category(category).await;
        assert!(
            matches!(result, Err(KnowledgeError::Validation(_))),
            "expected {name:?} to be rejected"
        );
    }
}

#[tokio::test]
async fn insert_category_requires_positive_id() {
    let graph = common::TestGraph::new().await;
    for id in [0, -1] {
        let category = NewCategory {
            id,
            name: "Reviewers".to_string(),
            description: None,
            parent_id: None,
            memory_weight: None,
            memory_bucket_id: None,
        };
        let result = graph.kg.insert_category(category).await;
        assert!(
            matches!(result, Err(KnowledgeError::Validation(_))),
            "expected id {id} to be rejected"
        );
    }
}

#[tokio::test]
async fn insert_category_rejects_self_parent() {
    let graph = common::TestGraph::new().await;
    let category = NewCategory {
        id: 99_006,
        name: "Reviewers".to_string(),
        description: None,
        parent_id: Some(99_006),
        memory_weight: None,
        memory_bucket_id: None,
    };
    let result = graph.kg.insert_category(category).await;
    assert!(matches!(result, Err(KnowledgeError::Validation(_))));
}

#[tokio::test]
async fn insert_category_accepts_existing_parent() {
    let graph = common::TestGraph::new().await;
    let category = NewCategory {
        id: 99_007,
        name: "Reviewers".to_string(),
        description: None,
        parent_id: Some(0),
        memory_weight: None,
        memory_bucket_id: None,
    };
    let inserted = graph.kg.insert_category(category).await.unwrap();
    assert_eq!(inserted.parent_id, Some(0));
}
