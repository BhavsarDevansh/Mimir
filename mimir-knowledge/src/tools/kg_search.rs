use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission};

use crate::KnowledgeGraph;
use crate::models::entity::EntityType;
use crate::queries::search::search_entities;

#[derive(Debug, Deserialize)]
struct KgSearchInput {
    query: String,
    entity_type: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    10
}

#[derive(Debug, Serialize)]
struct KgSearchOutput {
    query: String,
    results: Vec<crate::queries::search::SearchResult>,
}

fn parse_entity_type(s: &str) -> Option<EntityType> {
    match s.to_lowercase().as_str() {
        "person" => Some(EntityType::Person),
        "place" => Some(EntityType::Place),
        "event" => Some(EntityType::Event),
        "object" => Some(EntityType::Object),
        "concept" => Some(EntityType::Concept),
        "organization" => Some(EntityType::Organization),
        "activity" => Some(EntityType::Activity),
        "datetime" => Some(EntityType::DateTime),
        _ => None,
    }
}

pub struct KgSearchTool {
    kg: Arc<KnowledgeGraph>,
}

impl KgSearchTool {
    pub fn new(kg: Arc<KnowledgeGraph>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for KgSearchTool {
    fn name(&self) -> &str {
        "kg_search"
    }

    fn description(&self) -> &str {
        "Full-text search over the knowledge graph. Returns entities matching the query with their top facts."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query."
                },
                "entity_type": {
                    "type": "string",
                    "description": "Optional entity type filter (e.g., Person, Place)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results. Default 10, clamped 1–20.",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let input: KgSearchInput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments("kg_search", format!("invalid JSON args: {}", e))
        })?;

        // Input sanitization
        let query = input.query.trim();
        if query.is_empty() {
            return Err(ToolError::invalid_arguments(
                "kg_search",
                "query must be non-empty",
            ));
        }
        if query.len() > 500 {
            return Err(ToolError::invalid_arguments(
                "kg_search",
                "query exceeds 500 characters",
            ));
        }

        let limit = input.limit.clamp(1, 20);

        let entity_type_filter = input.entity_type.as_deref().and_then(parse_entity_type);

        let results = search_entities(self.kg.pool(), query, entity_type_filter, limit)
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_search", format!("database error: {}", e))
            })?;

        let output = KgSearchOutput {
            query: query.to_string(),
            results,
        };

        let result = serde_json::to_value(output).map_err(|e| {
            ToolError::execution_failed("kg_search", format!("serialization error: {}", e))
        })?;

        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
    }
}
