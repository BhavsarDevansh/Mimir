use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission};

use crate::KnowledgeGraph;
use crate::queries::entity::get_by_name;
use crate::queries::fact::{
    count_facts_by_relationship_subtree, count_facts_by_subject_filtered,
    get_facts_by_relationship_subtree, get_facts_by_subject_filtered,
};
use crate::tools::{entity_type_name, fact_status_name, source_type_name};

#[derive(Debug, Deserialize)]
struct KgQueryInput {
    entity_name: String,
    predicate: Option<String>,
    #[serde(default = "default_min_confidence")]
    min_confidence: f32,
    #[serde(default = "default_offset")]
    offset: i64,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    include_subtree: bool,
}

fn default_min_confidence() -> f32 {
    0.5
}
fn default_offset() -> i64 {
    0
}
fn default_limit() -> i64 {
    20
}

#[derive(Debug, Serialize)]
struct EntitySummary {
    id: u32,
    name: String,
    entity_type: String,
}

#[derive(Debug, Serialize)]
struct SourceSummary {
    source_type: String,
    extracted_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct FactDetail {
    id: u32,
    predicate: String,
    object_name: Option<String>,
    object_literal: Option<String>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    confidence: f32,
    status: String,
    inferred: bool,
    sources: Vec<SourceSummary>,
}

#[derive(Debug, Serialize)]
struct KgQueryOutput {
    entity: EntitySummary,
    facts: Vec<FactDetail>,
    total: usize,
    offset: i64,
    limit: i64,
}

pub struct KgQueryTool {
    kg: Arc<KnowledgeGraph>,
}

impl KgQueryTool {
    pub fn new(kg: Arc<KnowledgeGraph>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for KgQueryTool {
    fn name(&self) -> &str {
        "kg_query"
    }

    fn description(&self) -> &str {
        "Query facts about a specific entity by name. Returns verified facts (not pending) with confidence scores, temporal bounds, and provenance."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "entity_name": {
                    "type": "string",
                    "description": "Name of the entity to query."
                },
                "predicate": {
                    "type": "string",
                    "description": "Optional predicate filter. Only return facts with this predicate."
                },
                "min_confidence": {
                    "type": "number",
                    "description": "Minimum confidence threshold (0.0–1.0). Default 0.5.",
                    "minimum": 0.0,
                    "maximum": 1.0
                },
                "offset": {
                    "type": "integer",
                    "description": "Pagination offset. Default 0.",
                    "minimum": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum facts to return. Default 20, max 50.",
                    "minimum": 1,
                    "maximum": 50
                },
                "include_subtree": {
                    "type": "boolean",
                    "description": "If true, expand `predicate` to its relationship-type subtree: return facts whose type is `predicate` or any descendant in the relationship-type DAG. Requires `predicate`. Default false.",
                    "default": false
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
        let input: KgQueryInput = serde_json::from_value(args).map_err(|e| {
            ToolError::invalid_arguments("kg_query", format!("invalid JSON args: {}", e))
        })?;

        // Input sanitization
        let entity_name = input.entity_name.trim();
        if entity_name.is_empty() {
            return Err(ToolError::invalid_arguments(
                "kg_query",
                "entity_name must be non-empty",
            ));
        }
        if entity_name.len() > 200 {
            return Err(ToolError::invalid_arguments(
                "kg_query",
                "entity_name exceeds 200 characters",
            ));
        }

        if let Some(ref pred) = input.predicate {
            if pred.len() > 200 {
                return Err(ToolError::invalid_arguments(
                    "kg_query",
                    "predicate exceeds 200 characters",
                ));
            }
        }

        let min_confidence = input.min_confidence.clamp(0.0, 1.0);
        let offset = input.offset.max(0);
        let limit = input.limit.clamp(1, 50);
        let include_subtree = input.include_subtree;
        if include_subtree && input.predicate.is_none() {
            return Err(ToolError::invalid_arguments(
                "kg_query",
                "include_subtree requires a predicate",
            ));
        }

        // Resolve entity
        let candidates = get_by_name(self.kg.pool(), entity_name)
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_query", format!("database error: {}", e))
            })?;

        if candidates.is_empty() {
            return Ok(ToolOutput {
                error: Some("Entity not found".to_string()),
                ..Default::default()
            });
        }

        let best = &candidates[0];
        let subject_id = best.entity.id;

        // Resolve predicate if provided (read-only: do not create missing predicates)
        let relationship_type_id_opt = if let Some(ref pred) = input.predicate {
            let trimmed = pred.trim();
            if trimmed.is_empty() {
                return Err(ToolError::invalid_arguments(
                    "kg_query",
                    "predicate must be non-empty",
                ));
            }
            match self
                .kg
                .get_relationship_type_id(trimmed)
                .await
                .map_err(|e| {
                    ToolError::execution_failed("kg_query", format!("database error: {}", e))
                })? {
                Some(id) => Some(id),
                None => {
                    // Predicate does not exist: return empty result set
                    let output = KgQueryOutput {
                        entity: EntitySummary {
                            id: best.entity.id as u32,
                            name: best.entity.name.clone(),
                            entity_type: entity_type_name(best.entity.entity_type_id),
                        },
                        total: 0,
                        offset,
                        limit,
                        facts: Vec::new(),
                    };
                    let result = serde_json::to_value(output).map_err(|e| {
                        ToolError::execution_failed(
                            "kg_query",
                            format!("serialization error: {}", e),
                        )
                    })?;
                    return Ok(ToolOutput {
                        result: Some(result),
                        ..Default::default()
                    });
                }
            }
        } else {
            None
        };

        // Fetch facts: subtree expansion (predicate + descendants via a
        // recursive CTE) or an exact predicate match.
        let (facts, total) = if include_subtree {
            let Some(root_type_id) = relationship_type_id_opt else {
                return Err(ToolError::execution_failed(
                    "kg_query",
                    "include_subtree set but predicate was not resolved",
                ));
            };
            let facts = get_facts_by_relationship_subtree(
                self.kg.pool(),
                subject_id,
                root_type_id,
                min_confidence,
                limit,
            )
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_query", format!("database error: {}", e))
            })?;
            let total = count_facts_by_relationship_subtree(
                self.kg.pool(),
                subject_id,
                root_type_id,
                min_confidence,
            )
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_query", format!("database error: {}", e))
            })? as usize;
            (facts, total)
        } else {
            let facts = get_facts_by_subject_filtered(
                self.kg.pool(),
                subject_id,
                relationship_type_id_opt,
                min_confidence,
                offset,
                limit,
            )
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_query", format!("database error: {}", e))
            })?;
            let total = count_facts_by_subject_filtered(
                self.kg.pool(),
                subject_id,
                relationship_type_id_opt,
                min_confidence,
            )
            .await
            .map_err(|e| {
                ToolError::execution_failed("kg_query", format!("database error: {}", e))
            })? as usize;
            (facts, total)
        };

        let mut fact_details = Vec::with_capacity(facts.len());
        for f in facts {
            let predicate = self
                .kg
                .relationship_type_name(f.relationship_type_id)
                .await
                .unwrap_or_else(|| format!("predicate:{}", f.relationship_type_id));
            let sources = f
                .sources
                .into_iter()
                .map(|s| SourceSummary {
                    source_type: source_type_name(s.source_type_id),
                    extracted_at: s.extracted_at,
                })
                .collect();
            fact_details.push(FactDetail {
                id: f.id as u32,
                predicate,
                object_name: f.object_name,
                object_literal: f.object_literal,
                valid_from: f.valid_from,
                valid_until: f.valid_until,
                confidence: f.confidence,
                status: fact_status_name(f.fact_status_id),
                inferred: f.inferred,
                sources,
            });
        }

        let output = KgQueryOutput {
            entity: EntitySummary {
                id: best.entity.id as u32,
                name: best.entity.name.clone(),
                entity_type: entity_type_name(best.entity.entity_type_id),
            },
            total,
            offset: if include_subtree { 0 } else { offset },
            limit,
            facts: fact_details,
        };

        let result = serde_json::to_value(output).map_err(|e| {
            ToolError::execution_failed("kg_query", format!("serialization error: {}", e))
        })?;

        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
    }
}
