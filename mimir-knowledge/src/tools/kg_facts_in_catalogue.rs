use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission};

use crate::KnowledgeGraph;

#[derive(Debug, Deserialize)]
struct FactsInCatalogueInput {
    codes: Vec<i32>,
    #[serde(default)]
    match_all: bool,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Serialize)]
struct FactInCatalogueEntry {
    fact_id: i32,
    subject_name: String,
    relationship_type_name: String,
    object: String,
    confidence: f32,
    categories: Vec<i32>,
}

#[derive(Debug, Serialize)]
struct FactsInCatalogueOutput {
    facts: Vec<FactInCatalogueEntry>,
    total: usize,
}

pub struct KgFactsInCatalogueTool {
    kg: Arc<KnowledgeGraph>,
}

impl KgFactsInCatalogueTool {
    pub fn new(kg: Arc<KnowledgeGraph>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for KgFactsInCatalogueTool {
    fn name(&self) -> &str {
        "get_facts_in_catalogue"
    }

    fn description(&self) -> &str {
        "Retrieve facts that belong to specific catalogue categories. Provide category IDs (e.g., [200, 210]). Use match_all=true for facts in ALL categories (intersection), match_all=false for ANY (union)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "codes": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Category IDs to filter by."
                },
                "match_all": {
                    "type": "boolean",
                    "description": "If true, a fact must belong to ALL listed categories. If false, ANY category matches. Default false."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum facts to return. Default 20, max 50.",
                    "minimum": 1,
                    "maximum": 50
                }
            },
            "required": ["codes"]
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let input: FactsInCatalogueInput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments(
                "get_facts_in_catalogue",
                format!("invalid JSON args: {}", e),
            )
        })?;

        if input.codes.is_empty() {
            return Err(ToolError::invalid_arguments(
                "get_facts_in_catalogue",
                "codes must not be empty",
            ));
        }

        let limit = input.limit.clamp(1, 50);

        let facts = if input.match_all {
            self.kg
                .get_facts_matching_all_categories(&input.codes, limit)
                .await
                .map_err(|e| {
                    ToolError::execution_failed(
                        "get_facts_in_catalogue",
                        format!("database error: {}", e),
                    )
                })?
        } else {
            self.kg
                .get_facts_matching_any_categories(&input.codes, limit)
                .await
                .map_err(|e| {
                    ToolError::execution_failed(
                        "get_facts_in_catalogue",
                        format!("database error: {}", e),
                    )
                })?
        };

        let mut entries = Vec::with_capacity(facts.len());
        for f in facts {
            let object = f
                .object_name
                .clone()
                .or(f.object_literal.clone())
                .unwrap_or_default();
            let categories: Vec<i32> = f
                .categories
                .split(',')
                .filter_map(|s| s.parse::<i32>().ok())
                .collect();
            entries.push(FactInCatalogueEntry {
                fact_id: f.fact_id,
                subject_name: f.subject_name,
                relationship_type_name: f.relationship_type_name,
                object,
                confidence: f.confidence,
                categories,
            });
        }

        let output = FactsInCatalogueOutput {
            total: entries.len(),
            facts: entries,
        };

        let result = serde_json::to_value(output).map_err(|e| {
            ToolError::execution_failed(
                "get_facts_in_catalogue",
                format!("serialization error: {}", e),
            )
        })?;

        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
    }
}
