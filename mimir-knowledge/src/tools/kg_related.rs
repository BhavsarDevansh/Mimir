use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission};

use crate::KnowledgeGraph;
use crate::queries::entity::get_by_name;
use crate::queries::traverse::{TraversalEdge, traverse_graph};

#[derive(Debug, Deserialize)]
struct KgRelatedInput {
    entity_name: String,
    #[serde(default = "default_max_depth")]
    max_depth: i32,
    #[serde(default = "default_max_nodes")]
    max_nodes: i32,
    predicate_filter: Option<Vec<String>>,
}

fn default_max_depth() -> i32 {
    2
}
fn default_max_nodes() -> i32 {
    50
}

#[derive(Debug, Serialize)]
struct KgRelatedOutput {
    root_entity: String,
    nodes_found: usize,
    max_depth_reached: u32,
    edges: Vec<TraversalEdge>,
}

pub struct KgRelatedTool {
    kg: Arc<KnowledgeGraph>,
}

impl KgRelatedTool {
    pub fn new(kg: Arc<KnowledgeGraph>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for KgRelatedTool {
    fn name(&self) -> &str {
        "kg_related"
    }

    fn description(&self) -> &str {
        "Discover related entities via breadth-first traversal of the knowledge graph. Returns a bounded subgraph rooted at the named entity."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "entity_name": {
                    "type": "string",
                    "description": "Name of the root entity to traverse from."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum traversal depth. Default 2, clamped 1–5.",
                    "minimum": 1,
                    "maximum": 5
                },
                "max_nodes": {
                    "type": "integer",
                    "description": "Maximum nodes to visit. Default 50, clamped 1–200.",
                    "minimum": 1,
                    "maximum": 200
                },
                "predicate_filter": {
                    "type": "array",
                    "description": "Optional list of predicates to follow. Max 10 items.",
                    "items": { "type": "string" }
                }
            },
            "required": ["entity_name"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let input: KgRelatedInput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments("kg_related", format!("invalid JSON args: {}", e))
        })?;

        // Input sanitization
        let entity_name = input.entity_name.trim();
        if entity_name.is_empty() {
            return Err(ToolError::invalid_arguments(
                "kg_related",
                "entity_name must be non-empty",
            ));
        }
        if entity_name.len() > 200 {
            return Err(ToolError::invalid_arguments(
                "kg_related",
                "entity_name exceeds 200 characters",
            ));
        }

        let max_depth = input.max_depth.clamp(1, 5) as u32;
        let max_nodes = input.max_nodes.clamp(1, 200) as u32;

        // Resolve entity first (needed for early-return on missing predicates)
        let candidates = get_by_name(self.kg.pool(), entity_name)
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_related", format!("database error: {}", e))
            })?;

        if candidates.is_empty() {
            return Ok(ToolOutput {
                error: Some("Entity not found".to_string()),
                ..Default::default()
            });
        }

        let best = &candidates[0];
        let root_id = best.entity.id as u32;

        // Resolve predicate filters (read-only: do not create missing predicates)
        let mut predicate_ids: Vec<i16> = Vec::new();
        let user_requested_predicates = input
            .predicate_filter
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if let Some(filters) = input.predicate_filter {
            if filters.len() > 10 {
                return Err(ToolError::invalid_arguments(
                    "kg_related",
                    "predicate_filter exceeds 10 items",
                ));
            }
            for pred in &filters {
                if pred.len() > 200 {
                    return Err(ToolError::invalid_arguments(
                        "kg_related",
                        "predicate_filter item exceeds 200 characters",
                    ));
                }
                let trimmed = pred.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match self
                    .kg
                    .get_relationship_type_id(trimmed)
                    .await
                    .map_err(|e| {
                        ToolError::execution_failed("kg_related", format!("database error: {}", e))
                    })? {
                    Some(pid) => predicate_ids.push(pid),
                    None => continue, // skip missing predicates
                }
            }
        }

        // If the user explicitly requested predicates but none exist, return empty result.
        let predicate_filter = if predicate_ids.is_empty() {
            if user_requested_predicates {
                let output = KgRelatedOutput {
                    root_entity: best.entity.name.clone(),
                    nodes_found: 1,
                    max_depth_reached: 0,
                    edges: Vec::new(),
                };
                let result = serde_json::to_value(output).map_err(|e| {
                    ToolError::execution_failed("kg_related", format!("serialization error: {}", e))
                })?;
                return Ok(ToolOutput {
                    result: Some(result),
                    ..Default::default()
                });
            }
            None
        } else {
            Some(predicate_ids.as_slice())
        };

        let traversal = traverse_graph(
            self.kg.pool(),
            root_id,
            max_depth,
            max_nodes,
            predicate_filter,
        )
        .await
        .map_err(|e| ToolError::execution_failed("kg_related", format!("database error: {}", e)))?;

        let output = KgRelatedOutput {
            root_entity: best.entity.name.clone(),
            nodes_found: traversal.nodes_found,
            max_depth_reached: traversal.max_depth_reached,
            edges: traversal.edges,
        };

        let result = serde_json::to_value(output).map_err(|e| {
            ToolError::execution_failed("kg_related", format!("serialization error: {}", e))
        })?;

        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
    }
}
