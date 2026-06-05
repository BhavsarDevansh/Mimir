use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission};

use crate::KnowledgeGraph;

#[derive(Debug, Deserialize)]
struct ExpandCatalogueInput {
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Serialize)]
struct CategoryEntry {
    id: i32,
    name: String,
    description: Option<String>,
    fact_count: i64,
}

#[derive(Debug, Serialize)]
struct ExpandCatalogueOutput {
    target: Option<CategoryEntry>,
    children: Vec<CategoryEntry>,
}

pub struct KgExpandCatalogueTool {
    kg: Arc<KnowledgeGraph>,
}

impl KgExpandCatalogueTool {
    pub fn new(kg: Arc<KnowledgeGraph>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for KgExpandCatalogueTool {
    fn name(&self) -> &str {
        "expand_catalogue"
    }

    fn description(&self) -> &str {
        "Expand a category in the knowledge catalogue. Pass a category code (e.g., '200') to see its sub-categories, or omit/ pass null to see top-level categories."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": ["string", "null"],
                    "description": "Category code to expand (e.g., '200'). Pass null or omit for top-level categories."
                }
            },
            "required": []
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let input: ExpandCatalogueInput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments("expand_catalogue", format!("invalid JSON args: {}", e))
        })?;

        let parent_id = match input.code {
            Some(ref code) if code.trim().is_empty() || code == "root" || code == "null" => None,
            Some(ref code) => match code.parse::<i32>() {
                Ok(id) => Some(id),
                Err(_) => {
                    return Err(ToolError::invalid_arguments(
                        "expand_catalogue",
                        "code must be a numeric category ID or null",
                    ));
                }
            },
            None => None,
        };

        let children = self.kg.list_categories(parent_id).await.map_err(|e| {
            ToolError::execution_failed("expand_catalogue", format!("database error: {}", e))
        })?;

        let target = if let Some(id) = parent_id {
            self.kg
                .get_category(id)
                .await
                .map_err(|e| {
                    ToolError::execution_failed(
                        "expand_catalogue",
                        format!("database error: {}", e),
                    )
                })?
                .map(|c| CategoryEntry {
                    id: c.id,
                    name: c.name,
                    description: c.description,
                    fact_count: c.fact_count,
                })
        } else {
            None
        };

        let child_entries: Vec<CategoryEntry> = children
            .into_iter()
            .map(|c| CategoryEntry {
                id: c.id,
                name: c.name,
                description: c.description,
                fact_count: 0, // Could query per child but keeping it lightweight
            })
            .collect();

        let output = ExpandCatalogueOutput {
            target,
            children: child_entries,
        };

        let result = serde_json::to_value(output).map_err(|e| {
            ToolError::execution_failed("expand_catalogue", format!("serialization error: {}", e))
        })?;

        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
    }
}
