use std::sync::Arc;

use crate::types::{CategoryDetailResponse, CategoryResponse};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;

use crate::error;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListCategoriesQuery {
    #[serde(default)]
    parent: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCategoryBody {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<i32>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub memory_weight: Option<f32>,
    /// Memory bucket id (`memory_buckets` lookup); omit for General.
    #[serde(default)]
    pub memory_bucket_id: Option<i16>,
}

/// List categories.
pub async fn list_categories(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListCategoriesQuery>,
) -> Result<Json<Vec<CategoryResponse>>, Response> {
    let cats = state
        .knowledge_graph
        .list_categories(query.parent)
        .await
        .map_err(error::knowledge_error)?;

    let resp: Vec<CategoryResponse> = cats
        .into_iter()
        .map(|c| CategoryResponse {
            id: c.id,
            name: c.name,
            description: c.description,
            parent_id: c.parent_id,
            memory_weight: c.memory_weight,
            memory_bucket_id: c.memory_bucket_id,
        })
        .collect();

    Ok(Json(resp))
}

/// Show a single category with children.
pub async fn show_category(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<CategoryDetailResponse>, Response> {
    let cat = state
        .knowledge_graph
        .get_category(id)
        .await
        .map_err(error::knowledge_error)?;

    let cat = match cat {
        Some(c) => c,
        None => return Err(error::not_found("Category not found")),
    };

    let children = state
        .knowledge_graph
        .get_category_children(id)
        .await
        .map_err(error::knowledge_error)?;

    let resp = CategoryDetailResponse {
        id: cat.id,
        name: cat.name.clone(),
        description: cat.description.clone(),
        parent_id: cat.parent_id,
        memory_weight: cat.memory_weight,
        memory_bucket_id: cat.memory_bucket_id,
        fact_count: cat.fact_count,
        children: children
            .into_iter()
            .map(|c| CategoryResponse {
                id: c.id,
                name: c.name,
                description: c.description,
                parent_id: c.parent_id,
                memory_weight: c.memory_weight,
                memory_bucket_id: c.memory_bucket_id,
            })
            .collect(),
    };

    Ok(Json(resp))
}

/// Create a new category.
pub async fn create_category(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateCategoryBody>,
) -> Result<Json<mimir_knowledge::models::category::Category>, Response> {
    let new_cat = mimir_knowledge::models::category::NewCategory {
        id: body.id,
        name: body.name,
        description: body.description,
        parent_id: body.parent_id,
        memory_weight: body.memory_weight,
        memory_bucket_id: body.memory_bucket_id,
    };

    let cat = state
        .knowledge_graph
        .insert_category(new_cat)
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(cat))
}

/// Delete a category.
pub async fn delete_category(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<StatusCode, Response> {
    state
        .knowledge_graph
        .delete_category(id)
        .await
        .map_err(error::knowledge_error)?;

    Ok(StatusCode::NO_CONTENT)
}
