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
    #[error("Test fixture failed: {0}")]
    Fixture(String),
}

/// Copy the pre-migrated schema template to a destination path.
pub async fn prepare_from_template(db_path: &Path) -> Result<(), TestSupportError> {
    let template = template_path().await?;
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(template, db_path)
        .await
        .map_err(TestSupportError::Copy)?;
    Ok(())
}

/// Resolve a normalized relationship type name to a test fixture id.
///
/// Existing aliases resolve to their canonical id; an unknown name creates a
/// non-emitting fixture row in the same transaction as its self-alias.
pub async fn ensure_relationship_type(
    knowledge_graph: &KnowledgeGraph,
    name: &str,
) -> Result<i16, TestSupportError> {
    let normalized = name.trim();
    if normalized.is_empty() {
        return Err(TestSupportError::Fixture(
            "relationship type name cannot be empty".to_string(),
        ));
    }

    if let Some(id) = knowledge_graph.get_relationship_type_id(normalized).await? {
        return Ok(id);
    }

    let mut tx = knowledge_graph.pool().begin().await?;
    let id: i16 = sqlx::query_scalar(
        "INSERT INTO relationship_types (name, description, node_kind, emit_eligible) \
         VALUES (?, ?, 'alias', FALSE) \
         ON CONFLICT (name) DO UPDATE SET name = relationship_types.name RETURNING id",
    )
    .bind(normalized)
    .bind(format!("Test fixture relationship type: {normalized}"))
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id) \
         VALUES (?, ?)",
    )
    .bind(normalized)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
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
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::path::Path;
    use std::str::FromStr;

    async fn migration_count(db_path: &Path) -> i64 {
        let options =
            SqliteConnectOptions::from_str(db_path.to_str().expect("database path is valid UTF-8"))
                .unwrap();
        let pool = SqlitePoolOptions::new()
            .connect_with(options)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        pool.close().await;
        count
    }

    #[tokio::test]
    async fn prepare_from_template_copies_migrated_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("knowledge.db");
        let expected_path = dir.path().join("expected.db");
        let expected_knowledge_graph = KnowledgeGraph::init(&expected_path).await.unwrap();
        let expected_migration_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
                .fetch_one(expected_knowledge_graph.pool())
                .await
                .unwrap();

        let template_path = crate::template_path().await.unwrap();
        assert_eq!(
            migration_count(template_path).await,
            expected_migration_count
        );

        crate::prepare_from_template(&db_path).await.unwrap();
        assert_eq!(migration_count(&db_path).await, expected_migration_count);
    }

    #[tokio::test]
    async fn prepare_from_template_creates_destination_parent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("knowledge.db");

        crate::prepare_from_template(&db_path).await.unwrap();

        assert!(db_path.is_file());
    }

    #[tokio::test]
    async fn template_is_shared_across_copies() {
        let first = crate::template_path().await.unwrap();
        let second = crate::template_path().await.unwrap();

        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn ensure_relationship_type_creates_and_reuses_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("knowledge.db");
        let kg = KnowledgeGraph::init(&db_path).await.unwrap();

        let first = crate::ensure_relationship_type(&kg, "fixture_relationship")
            .await
            .unwrap();
        let second = crate::ensure_relationship_type(&kg, "fixture_relationship")
            .await
            .unwrap();

        assert_eq!(first, second);
        assert!(
            kg.resolve_canonical_relationship_type("fixture_relationship")
                .await
                .is_err()
        );
    }
}
