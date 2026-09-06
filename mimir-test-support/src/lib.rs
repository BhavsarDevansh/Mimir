#![deny(unsafe_code)]
//! Test-only helpers for fast, isolated knowledge-graph databases.

use mimir_knowledge::{KnowledgeError, KnowledgeGraph};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::sync::{Mutex, OnceCell};

struct TemplateSchema {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

/// Errors returned by the development-only schema-template fixture.
#[derive(Debug, Error)]
pub enum TestSupportError {
    #[error("Template database initialisation failed: {0}")]
    Template(#[from] KnowledgeError),
    #[error("Template database query failed: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Template database copy failed: {0}")]
    Copy(#[from] std::io::Error),
}

/// Initialise a clean knowledge graph by copying the pre-migrated template.
pub async fn init_from_template(db_path: &Path) -> Result<KnowledgeGraph, TestSupportError> {
    let template = template_path().await?;
    tokio::fs::copy(template, db_path)
        .await
        .map_err(TestSupportError::Copy)?;
    KnowledgeGraph::init(db_path)
        .await
        .map_err(TestSupportError::Template)
}

/// Return the process-local path to the pre-migrated SQLite schema template.
async fn template_path() -> Result<&'static Path, TestSupportError> {
    static TEMPLATE: OnceCell<TemplateSchema> = OnceCell::const_new();
    static TEMPLATE_INIT: OnceCell<Mutex<()>> = OnceCell::const_new();
    let _init_lock = TEMPLATE_INIT
        .get_or_init(|| async { Mutex::new(()) })
        .await
        .lock()
        .await;
    if let Some(template) = TEMPLATE.get() {
        return Ok(template.path.as_path());
    }

    let template = build_template().await?;
    let _ = TEMPLATE.set(template);

    Ok(TEMPLATE
        .get()
        .expect("template was just set")
        .path
        .as_path())
}

async fn build_template() -> Result<TemplateSchema, TestSupportError> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("migrated.db");
    let path = dir.path().join("schema-template.db");
    let knowledge_graph = KnowledgeGraph::init(&source).await?;
    let escaped = path.to_string_lossy().replace('\'', "''");
    sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{escaped}'")))
        .execute(knowledge_graph.pool())
        .await?;
    let pool = knowledge_graph.pool().clone();
    drop(knowledge_graph);
    pool.close().await;
    tokio::fs::remove_file(&source).await?;

    Ok(TemplateSchema { _dir: dir, path })
}

#[cfg(test)]
mod tests {
    use mimir_knowledge::KnowledgeGraph;

    #[tokio::test]
    async fn init_from_template_copies_migrated_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("knowledge.db");
        let expected_path = dir.path().join("expected.db");
        let expected_knowledge_graph = KnowledgeGraph::init(&expected_path).await.unwrap();
        let expected_migration_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
                .fetch_one(expected_knowledge_graph.pool())
                .await
                .unwrap();

        let knowledge_graph = crate::init_from_template(&db_path).await.unwrap();

        let migration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(knowledge_graph.pool())
            .await
            .unwrap();
        assert_eq!(migration_count, expected_migration_count);
    }

    #[tokio::test]
    async fn template_is_shared_across_copies() {
        let first = crate::template_path().await.unwrap();
        let second = crate::template_path().await.unwrap();

        assert_eq!(first, second);
    }
}
