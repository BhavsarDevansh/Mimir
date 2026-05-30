//! Tests for `KnowledgeGraph::init()` using the public API and a temporary directory.

use chrono::Utc;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::clock::MockClock;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn init_creates_db_file_and_applies_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");

    assert!(!db_path.exists());

    let kg = KnowledgeGraph::init(&db_path).await.unwrap();

    assert!(db_path.exists());

    // Verify pool is functional by running a simple query.
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sqlite_master")
        .fetch_one(kg.pool())
        .await
        .unwrap();
    assert!(count > 0);
}

#[tokio::test]
async fn init_with_mock_clock_returns_deterministic_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");

    let fixed = Utc::now();
    let clock = Arc::new(MockClock::new(fixed));

    let kg = KnowledgeGraph::init_with_clock(&db_path, clock.clone())
        .await
        .unwrap();

    assert_eq!(kg.now(), fixed);

    clock.advance_seconds(3600);
    assert_eq!(kg.now(), fixed + chrono::Duration::seconds(3600));
}

#[tokio::test]
async fn init_idempotent_on_existing_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("knowledge.db");

    let kg1 = KnowledgeGraph::init(&db_path).await.unwrap();
    let (count1,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sqlite_master")
        .fetch_one(kg1.pool())
        .await
        .unwrap();

    let kg2 = KnowledgeGraph::init(&db_path).await.unwrap();
    let (count2,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sqlite_master")
        .fetch_one(kg2.pool())
        .await
        .unwrap();

    assert_eq!(count1, count2);
}
