//! Category model for the Dewey Decimal-style taxonomy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A node in the hierarchical category taxonomy.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Category {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i32>,
    pub memory_weight: Option<f32>,
    pub created_at: DateTime<Utc>,
}

/// Input for inserting a new category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCategory {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i32>,
    pub memory_weight: Option<f32>,
}

/// A category with its count of associated facts.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct CategoryWithCount {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i32>,
    pub memory_weight: Option<f32>,
    pub created_at: DateTime<Utc>,
    pub fact_count: i64,
}

/// A category with its direct children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryTreeNode {
    pub category: Category,
    pub children: Vec<Category>,
}

/// A fact with its associated categories.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct FactWithCategories {
    pub fact_id: i32,
    pub subject_name: String,
    pub relationship_type_name: String,
    pub object_name: Option<String>,
    pub object_literal: Option<String>,
    pub confidence: f32,
    pub categories: String, // comma-separated category IDs
}
