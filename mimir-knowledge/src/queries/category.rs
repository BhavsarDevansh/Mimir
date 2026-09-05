//! Category CRUD, tree queries, and fact-category lookups.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::category::{Category, CategoryWithCount, FactWithCategories, NewCategory};
use crate::models::fact::FactStatus;
use crate::models::memory::MemoryBucket;
use std::collections::BTreeSet;

/// List categories, optionally filtered by parent.
pub async fn list_categories(
    pool: &SqlitePool,
    parent_id: Option<i32>,
) -> Result<Vec<Category>, KnowledgeError> {
    let rows = match parent_id {
        Some(pid) => sqlx::query_as::<_, Category>(
            "SELECT id, name, description, parent_id, memory_weight, memory_bucket_id, created_at \
                 FROM categories WHERE parent_id = ? ORDER BY id",
        )
        .bind(pid)
        .fetch_all(pool)
        .await?,
        None => sqlx::query_as::<_, Category>(
            "SELECT id, name, description, parent_id, memory_weight, memory_bucket_id, created_at \
                 FROM categories WHERE parent_id IS NULL ORDER BY id",
        )
        .fetch_all(pool)
        .await?,
    };
    Ok(rows)
}

/// List every category in ID order for rendering or auditing the full tree.
pub async fn list_all_categories(pool: &SqlitePool) -> Result<Vec<Category>, KnowledgeError> {
    let rows = sqlx::query_as::<_, Category>(
        "SELECT id, name, description, parent_id, memory_weight, memory_bucket_id, created_at \
         FROM categories ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get a single category with its fact count.
pub async fn get_category(
    pool: &SqlitePool,
    id: i32,
) -> Result<Option<CategoryWithCount>, KnowledgeError> {
    let row: Option<CategoryWithCount> = sqlx::query_as(
        "SELECT c.id, c.name, c.description, c.parent_id, c.memory_weight, c.memory_bucket_id, c.created_at, \
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
        "SELECT id, name, description, parent_id, memory_weight, memory_bucket_id, created_at \
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
    if new.parent_id == Some(new.id) {
        return Err(KnowledgeError::Validation(
            "Category cannot be its own parent".to_string(),
        ));
    }
    if let Some(bucket_id) = new.memory_bucket_id {
        if MemoryBucket::try_from(bucket_id).is_err() {
            return Err(KnowledgeError::Validation(format!(
                "Unknown memory bucket id {bucket_id}; expected 1-5"
            )));
        }
    }
    let category: Category = sqlx::query_as(
        "INSERT INTO categories (id, name, description, parent_id, memory_weight, memory_bucket_id) \
         VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, name, description, parent_id, memory_weight, memory_bucket_id, created_at",
    )
    .bind(new.id)
    .bind(&new.name)
    .bind(&new.description)
    .bind(new.parent_id)
    .bind(new.memory_weight)
    .bind(new.memory_bucket_id)
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
        "SELECT c.id, c.name, c.description, c.parent_id, c.memory_weight, c.memory_bucket_id, c.created_at \
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

/// Resolve a natural-language alias to a category id.
pub async fn resolve_category_alias(
    pool: &SqlitePool,
    alias: &str,
) -> Result<Option<i32>, KnowledgeError> {
    let Some(normalized) = crate::normalize_alias(alias) else {
        return Ok(None);
    };
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT category_id FROM category_aliases WHERE alias = ?")
            .bind(&normalized)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// List category aliases, optionally filtered by category id.
pub async fn list_category_aliases(
    pool: &SqlitePool,
    category_id: Option<i32>,
) -> Result<Vec<crate::models::category::CategoryAlias>, KnowledgeError> {
    let rows = match category_id {
        Some(cid) => sqlx::query_as::<_, crate::models::category::CategoryAlias>(
            "SELECT alias, category_id FROM category_aliases WHERE category_id = ? ORDER BY alias",
        )
        .bind(cid)
        .fetch_all(pool)
        .await?,
        None => {
            sqlx::query_as::<_, crate::models::category::CategoryAlias>(
                "SELECT alias, category_id FROM category_aliases ORDER BY alias",
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// Insert a category alias. Idempotent for the same alias→category mapping;
/// rejects empty aliases, unknown category ids, and rebinding an existing alias
/// to a different category (returns `Validation`).
pub async fn insert_category_alias(
    pool: &SqlitePool,
    alias: &str,
    category_id: i32,
) -> Result<(), KnowledgeError> {
    let Some(normalized) = crate::normalize_alias(alias) else {
        return Err(KnowledgeError::Validation(
            "category alias cannot be empty".to_string(),
        ));
    };

    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE id = ?")
        .bind(category_id)
        .fetch_one(pool)
        .await?;
    if exists == 0 {
        return Err(KnowledgeError::CategoryNotFound(category_id));
    }

    let mut tx = pool.begin().await?;
    // Atomic insert-then-resolve avoids the SELECT-then-INSERT race under
    // DEFERRED isolation: whichever concurrent writer inserts first wins, and
    // the subsequent read deterministically returns the canonical mapping so we
    // surface a `Validation` error (not a raw UNIQUE-constraint failure) on
    // rebind attempts.
    sqlx::query("INSERT OR IGNORE INTO category_aliases (alias, category_id) VALUES (?, ?)")
        .bind(&normalized)
        .bind(category_id)
        .execute(&mut *tx)
        .await?;

    let mapped: Option<(i32,)> =
        sqlx::query_as("SELECT category_id FROM category_aliases WHERE alias = ?")
            .bind(&normalized)
            .fetch_optional(&mut *tx)
            .await?;
    match mapped {
        // Idempotent: the alias now maps to the requested category.
        Some((existing_id,)) if existing_id == category_id => {}
        // Rebinding an existing alias to a different category is rejected so
        // callers do not silently keep the original mapping.
        Some((existing_id,)) => {
            return Err(KnowledgeError::Validation(format!(
                "category alias '{}' is already mapped to category {}",
                normalized, existing_id
            )));
        }
        None => {
            return Err(KnowledgeError::Validation(format!(
                "category alias '{}' could not be inserted",
                normalized
            )));
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Return all descendant category ids of `root_id` (exclusive of the root)
/// via a recursive CTE over the `categories.parent_id` tree.
pub async fn get_descendant_category_ids(
    pool: &SqlitePool,
    root_id: i32,
) -> Result<Vec<i32>, KnowledgeError> {
    let rows: Vec<(i32,)> = sqlx::query_as(
        r#"WITH RECURSIVE descendants(id) AS (
             SELECT id FROM categories WHERE parent_id = ?
             UNION
             SELECT c.id FROM categories c
             JOIN descendants d ON c.parent_id = d.id
             )
             SELECT id FROM descendants ORDER BY id"#,
    )
    .bind(root_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Get facts anywhere in a category subtree (root + all descendants).
pub async fn get_facts_in_category_subtree(
    pool: &SqlitePool,
    root_id: i32,
    limit: i64,
) -> Result<Vec<FactWithCategories>, KnowledgeError> {
    let mut ids = get_descendant_category_ids(pool, root_id).await?;
    ids.push(root_id);
    get_facts_matching_any_categories(pool, &ids, limit).await
}
