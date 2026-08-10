//! Knowledge-graph category commands: list, show, create, delete.

use mimir_api_types::{CategoryDetailResponse, CategoryResponse};

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// List knowledge graph categories.
    pub async fn kb_categories(
        &self,
        parent: Option<i32>,
    ) -> Result<Vec<CategoryResponse>, ClientError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(p) = parent {
            params.push(("parent", p.to_string()));
        }
        self.get_json(&self.url("kb/categories"), &params).await
    }

    /// Show a single category with its children.
    pub async fn kb_category_show(&self, id: i32) -> Result<CategoryDetailResponse, ClientError> {
        self.get_json(&self.url(&format!("kb/categories/{id}")), &())
            .await
    }

    /// Create a new knowledge graph category.
    pub async fn kb_category_create(
        &self,
        id: i32,
        name: String,
        parent_id: Option<i32>,
        description: Option<String>,
        memory_weight: Option<f32>,
    ) -> Result<CategoryResponse, ClientError> {
        let body = serde_json::json!({
            "id": id,
            "name": name,
            "parent_id": parent_id,
            "description": description,
            "memory_weight": memory_weight,
        });
        self.post_json(&self.url("kb/categories"), &body).await
    }

    /// Delete a knowledge graph category.
    pub async fn kb_category_delete(&self, id: i32) -> Result<(), ClientError> {
        Self::check_status(
            self.client
                .delete(self.url(&format!("kb/categories/{id}")))
                .send()
                .await?,
        )
        .await
    }
}
