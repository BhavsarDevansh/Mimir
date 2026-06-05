//! Category CRUD, tree queries, and fact-category lookups.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::category::{Category, CategoryWithCount, FactWithCategories, NewCategory};
use crate::models::fact::FactStatus;
use std::collections::BTreeSet;

/// List categories, optionally filtered by parent.
pub async fn list_categories(
    pool: &SqlitePool,
    parent_id: Option<i32>,
) -> Result<Vec<Category>, KnowledgeError> {
    let rows = match parent_id {
        Some(pid) => {
            sqlx::query_as::<_, Category>(
                "SELECT id, name, description, parent_id, memory_weight, created_at \
                 FROM categories WHERE parent_id = ? ORDER BY id",
            )
            .bind(pid)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, Category>(
                "SELECT id, name, description, parent_id, memory_weight, created_at \
                 FROM categories WHERE parent_id IS NULL ORDER BY id",
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// Get a single category with its fact count.
pub async fn get_category(
    pool: &SqlitePool,
    id: i32,
) -> Result<Option<CategoryWithCount>, KnowledgeError> {
    let row: Option<CategoryWithCount> = sqlx::query_as(
        "SELECT c.id, c.name, c.description, c.parent_id, c.memory_weight, c.created_at, \
                COUNT(fc.fact_id) as fact_count \
         FROM categories c \
         LEFT JOIN fact_categories fc ON fc.category_id = c.id \
         WHERE c.id = ? \
         GROUP BY c.id",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Get direct children of a category.
pub async fn get_children(
    pool: &SqlitePool,
    parent_id: i32,
) -> Result<Vec<Category>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Category>(
        "SELECT id, name, description, parent_id, memory_weight, created_at \
         FROM categories WHERE parent_id = ? ORDER BY id",
    )
    .bind(parent_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Insert a new category.
pub async fn insert_category(
    pool: &SqlitePool,
    new: &NewCategory,
    _now: DateTime<Utc>,
) -> Result<Category, KnowledgeError> {
    let category: Category = sqlx::query_as(
        "INSERT INTO categories (id, name, description, parent_id, memory_weight) \
         VALUES (?, ?, ?, ?, ?) \
         RETURNING id, name, description, parent_id, memory_weight, created_at",
    )
    .bind(new.id)
    .bind(&new.name)
    .bind(&new.description)
    .bind(new.parent_id)
    .bind(new.memory_weight)
    .fetch_one(pool)
    .await?;
    Ok(category)
}

/// Delete a category if it has no children and no facts.
pub async fn delete_category(pool: &SqlitePool, id: i32) -> Result<(), KnowledgeError> {
    let child_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE parent_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;

    if child_count > 0 {
        return Err(KnowledgeError::Validation(format!(
            "Category {} has {} children and cannot be deleted",
            id, child_count
        )));
    }

    let fact_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM fact_categories WHERE category_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;

    if fact_count > 0 {
        return Err(KnowledgeError::Validation(format!(
            "Category {} has {} associated facts and cannot be deleted",
            id, fact_count
        )));
    }

    sqlx::query("DELETE FROM categories WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Get all categories for a fact.
pub async fn get_categories_for_fact(
    pool: &SqlitePool,
    fact_id: i32,
) -> Result<Vec<Category>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Category>(
        "SELECT c.id, c.name, c.description, c.parent_id, c.memory_weight, c.created_at \
         FROM categories c \
         JOIN fact_categories fc ON fc.category_id = c.id \
         WHERE fc.fact_id = ? \
         ORDER BY c.id",
    )
    .bind(fact_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get facts in a category with their full category lists.
pub async fn get_facts_in_category(
    pool: &SqlitePool,
    category_id: i32,
    limit: i64,
) -> Result<Vec<FactWithCategories>, KnowledgeError> {
    let rows = sqlx::query_as::<_, FactWithCategories>(
        "SELECT f.id as fact_id, s.name as subject_name, rt.name as relationship_type_name, \
                COALESCE(o.name, f.object_literal) as object_name, f.object_literal, f.confidence, \
                GROUP_CONCAT(fc2.category_id) as categories \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         JOIN fact_categories fc ON fc.fact_id = f.id \
         LEFT JOIN fact_categories fc2 ON fc2.fact_id = f.id \
         WHERE fc.category_id = ? AND f.fact_status_id NOT IN (?, ?) AND f.pending_confirmation = 0 \
         GROUP BY f.id \
         ORDER BY f.confidence DESC \
         LIMIT ?"
    )
    .bind(category_id)
    .bind(FactStatus::Superseded as i16)
    .bind(FactStatus::Forgotten as i16)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get facts matching ALL provided category IDs (intersection).
pub async fn get_facts_matching_all_categories(
    pool: &SqlitePool,
    category_ids: &[i32],
    limit: i64,
) -> Result<Vec<FactWithCategories>, KnowledgeError> {
    let unique_ids: Vec<i32> = category_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if unique_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = unique_ids.iter().map(|_| "?".to_string()).collect();
    let in_clause = placeholders.join(",");
    let count = unique_ids.len() as i64;

    let sql = format!(
        "SELECT f.id as fact_id, s.name as subject_name, rt.name as relationship_type_name, \
                COALESCE(o.name, f.object_literal) as object_name, f.object_literal, f.confidence, \
                GROUP_CONCAT(fc2.category_id) as categories \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         JOIN fact_categories fc ON fc.fact_id = f.id \
         LEFT JOIN fact_categories fc2 ON fc2.fact_id = f.id \
         WHERE fc.category_id IN ({}) AND f.fact_status_id NOT IN (5, 6) AND f.pending_confirmation = 0 \
         GROUP BY f.id \
         HAVING COUNT(DISTINCT fc.category_id) = ? \
         ORDER BY f.confidence DESC \
         LIMIT ?",
        in_clause
    );

    let mut query = sqlx::query_as::<_, FactWithCategories>(sqlx::AssertSqlSafe(&*sql));
    for &id in &unique_ids {
        query = query.bind(id);
    }
    query = query.bind(count).bind(limit);

    let rows = query.fetch_all(pool).await?;
    Ok(rows)
}

/// Get facts matching ANY provided category IDs (union).
pub async fn get_facts_matching_any_categories(
    pool: &SqlitePool,
    category_ids: &[i32],
    limit: i64,
) -> Result<Vec<FactWithCategories>, KnowledgeError> {
    if category_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = category_ids.iter().map(|_| "?".to_string()).collect();
    let in_clause = placeholders.join(",");

    let sql = format!(
        "SELECT f.id as fact_id, s.name as subject_name, rt.name as relationship_type_name, \
                COALESCE(o.name, f.object_literal) as object_name, f.object_literal, f.confidence, \
                GROUP_CONCAT(fc2.category_id) as categories \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         JOIN fact_categories fc ON fc.fact_id = f.id \
         LEFT JOIN fact_categories fc2 ON fc2.fact_id = f.id \
         WHERE fc.category_id IN ({}) AND f.fact_status_id NOT IN (5, 6) AND f.pending_confirmation = 0 \
         GROUP BY f.id \
         ORDER BY f.confidence DESC \
         LIMIT ?",
        in_clause
    );

    let mut query = sqlx::query_as::<_, FactWithCategories>(sqlx::AssertSqlSafe(&*sql));
    for &id in category_ids {
        query = query.bind(id);
    }
    query = query.bind(limit);

    let rows = query.fetch_all(pool).await?;
    Ok(rows)
}
