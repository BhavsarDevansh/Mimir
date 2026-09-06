//! Deterministic retrieval over the knowledge graph and conversation history.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::future::join_all;
use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolProgress};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::KnowledgeGraph;
use crate::retrieval::types::{
    ConversationSnippet, RetrievedContext, RetrievedEntity, RetrievedFact, RetrievedRelation,
};
use crate::tools::{KgQueryTool, KgRelatedTool, KgSearchTool};

/// Parse an RFC 3339 JSON string as a UTC timestamp.
fn parse_utc(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn same_text(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_lowercase)
        .eq(right.chars().flat_map(char::to_lowercase))
}

#[derive(Debug, Clone, PartialEq)]
enum RetrievalStep {
    KgSearch { query: String },
    ConversationSearch { query: String },
    KgQuery { entity_name: String },
    KgRelated { entity_name: String },
}

/// Executes a fixed retrieval plan in Rust.
///
/// The retrieval LLM is deliberately not part of this type: it has no ability
/// to select tools, repeat queries, or decide when retrieval ends.
pub struct RetrievalAgent {
    kg_search: KgSearchTool,
    kg_query: KgQueryTool,
    kg_related: KgRelatedTool,
    conversation_search: mimir_core::tools::SearchConversationHistoryTool,
    progress: Option<tokio::sync::mpsc::Sender<ToolProgress>>,
}

impl RetrievalAgent {
    /// Create a deterministic retriever backed by the shared stores.
    pub fn new(
        kg: Arc<KnowledgeGraph>,
        context_manager: Arc<mimir_core::context::ContextManager>,
    ) -> Self {
        Self {
            kg_search: KgSearchTool::new(Arc::clone(&kg)),
            kg_query: KgQueryTool::new(Arc::clone(&kg)),
            kg_related: KgRelatedTool::new(Arc::clone(&kg)),
            conversation_search: mimir_core::tools::SearchConversationHistoryTool::new(
                context_manager,
            ),
            progress: None,
        }
    }

    /// Attach a progress channel so each deterministic retrieval step is reported.
    pub fn with_progress(mut self, progress: tokio::sync::mpsc::Sender<ToolProgress>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Execute deterministic retrieval for `task`.
    pub async fn retrieve(&self, task: &str) -> Result<RetrievedContext, RetrievalAgentError> {
        let task = task.trim();
        if task.is_empty() {
            return Err(RetrievalAgentError::EmptyTask);
        }

        let initial_steps = Self::plan_initial_steps(task);
        let initial_results = self.execute_steps(&initial_steps).await;
        let mut context = RetrievedContext::default();
        let mut candidates = Vec::new();

        for (step, (tool_name, result)) in initial_steps.iter().zip(initial_results) {
            if let (RetrievalStep::KgSearch { .. }, Ok(output)) = (step, &result) {
                if let Some(value) = &output.result {
                    Self::collect_candidates(value, &mut candidates);
                }
            }
            Self::record_step(tool_name, result, &mut context);
        }

        let follow_up_steps = Self::plan_follow_up_steps(&candidates);
        let follow_up_results = self.execute_steps(&follow_up_steps).await;
        for (_step, (tool_name, result)) in follow_up_steps.iter().zip(follow_up_results) {
            Self::record_step(tool_name, result, &mut context);
        }

        context.steps_executed =
            u16::try_from(initial_steps.len() + follow_up_steps.len()).unwrap_or(u16::MAX);
        context.finish_reason = Some("completed".to_string());
        info!(
            steps = context.steps_executed,
            "deterministic retrieval completed"
        );
        Ok(context)
    }

    /// Build the initial fixed plan: one entity search per task token and one
    /// conversation search for the task.
    fn plan_initial_steps(task: &str) -> Vec<RetrievalStep> {
        let mut steps = Vec::new();
        let task_without_possessives = task.replace("'s", " ").replace("’s", " ");
        for token in task_without_possessives.split(|character: char| !character.is_alphanumeric())
        {
            if token.is_empty()
                || steps.iter().any(|step| {
                    matches!(step, RetrievalStep::KgSearch { query }
                        if same_text(query, token))
                })
            {
                continue;
            }
            steps.push(RetrievalStep::KgSearch {
                query: token.to_string(),
            });
        }
        steps.push(RetrievalStep::ConversationSearch {
            query: task.to_string(),
        });
        steps
    }

    /// Query facts and relationships once for every distinct search candidate.
    fn plan_follow_up_steps(candidates: &[String]) -> Vec<RetrievalStep> {
        candidates
            .iter()
            .flat_map(|entity_name| {
                [
                    RetrievalStep::KgQuery {
                        entity_name: entity_name.clone(),
                    },
                    RetrievalStep::KgRelated {
                        entity_name: entity_name.clone(),
                    },
                ]
            })
            .collect()
    }

    /// Extract distinct entity names from `kg_search` output.
    fn collect_candidates(result: &Value, candidates: &mut Vec<String>) {
        for match_result in result
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = match_result
                .get("entity")
                .and_then(|entity| entity.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            if !candidates
                .iter()
                .any(|candidate| same_text(candidate, name))
            {
                candidates.push(name.to_string());
            }
        }
    }

    /// Execute every step concurrently. Tool failures are logged and omitted
    /// from accumulated context, but never prevent other steps from running.
    async fn execute_steps(
        &self,
        steps: &[RetrievalStep],
    ) -> Vec<(&'static str, Result<ToolOutput, ToolError>)> {
        let futures = steps.iter().map(|step| self.execute_step(step));
        let results = join_all(futures).await;
        debug!(steps = steps.len(), "executed retrieval steps");
        results
    }

    async fn execute_step(
        &self,
        step: &RetrievalStep,
    ) -> (&'static str, Result<ToolOutput, ToolError>) {
        let (name, display_name) = match step {
            RetrievalStep::KgSearch { .. } => ("kg_search", "KG Search"),
            RetrievalStep::ConversationSearch { .. } => {
                ("search_conversation_history", "Search Conversation History")
            }
            RetrievalStep::KgQuery { .. } => ("kg_query", "KG Query"),
            RetrievalStep::KgRelated { .. } => ("kg_related", "KG Related"),
        };

        if let Some(progress) = &self.progress {
            let _ = progress
                .send(ToolProgress::Started {
                    name: name.to_string(),
                    display_name: display_name.to_string(),
                })
                .await;
        }

        let result = self.execute_tool(step).await;
        let result_text = match &result {
            Ok(output) => output.to_display_text(),
            Err(error) => error.to_string(),
        };
        if let Some(progress) = &self.progress {
            let _ = progress
                .send(ToolProgress::Finished {
                    name: name.to_string(),
                    display_name: display_name.to_string(),
                    result: result_text,
                })
                .await;
        }

        (name, result)
    }

    async fn execute_tool(&self, step: &RetrievalStep) -> Result<ToolOutput, ToolError> {
        match step {
            RetrievalStep::KgSearch { query } => {
                self.kg_search
                    .execute(serde_json::json!({"query": query, "limit": 10}))
                    .await
            }
            RetrievalStep::ConversationSearch { query } => {
                self.conversation_search
                    .execute(serde_json::json!({"query": query, "limit": 20}))
                    .await
            }
            RetrievalStep::KgQuery { entity_name } => {
                self.kg_query
                    .execute(serde_json::json!({"entity_name": entity_name}))
                    .await
            }
            RetrievalStep::KgRelated { entity_name } => {
                self.kg_related
                    .execute(serde_json::json!({"entity_name": entity_name}))
                    .await
            }
        }
    }

    fn record_step(
        tool_name: &str,
        result: Result<ToolOutput, ToolError>,
        context: &mut RetrievedContext,
    ) {
        match result {
            Ok(output) => {
                if let Some(value) = &output.result {
                    Self::accumulate_result(tool_name, value, context);
                } else if let Some(error) = &output.error {
                    warn!(tool = tool_name, "retrieval tool returned error: {}", error);
                }
            }
            Err(error) => {
                warn!(tool = tool_name, "retrieval tool failed: {}", error);
            }
        }
    }

    fn accumulate_result(tool_name: &str, result: &Value, context: &mut RetrievedContext) {
        match tool_name {
            "kg_query" => Self::accumulate_kg_query(result, context),
            "kg_related" => Self::accumulate_kg_related(result, context),
            "kg_search" => Self::accumulate_kg_search(result, context),
            "search_conversation_history" => Self::accumulate_conversation(result, context),
            _ => {}
        }
    }

    fn parse_retrieved_fact(fact: &Value) -> Option<RetrievedFact> {
        Some(RetrievedFact {
            predicate: fact.get("predicate")?.as_str()?.to_string(),
            object_name: fact
                .get("object_name")
                .and_then(Value::as_str)
                .map(String::from),
            object_literal: fact
                .get("object_literal")
                .and_then(Value::as_str)
                .map(String::from),
            confidence: fact
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32,
            valid_from: fact.get("valid_from").and_then(parse_utc),
            valid_until: fact.get("valid_until").and_then(parse_utc),
            status: fact
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("Active")
                .to_string(),
            inferred: fact
                .get("inferred")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn accumulate_kg_query(result: &Value, context: &mut RetrievedContext) {
        let entity_name = result
            .get("entity")
            .and_then(|entity| entity.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown");
        let entity_type = result
            .get("entity")
            .and_then(|entity| entity.get("entity_type"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown");
        let facts = result
            .get("facts")
            .and_then(Value::as_array)
            .map(|facts| {
                facts
                    .iter()
                    .filter_map(Self::parse_retrieved_fact)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Self::merge_entity_facts(context, entity_name, entity_type, facts);
    }

    fn merge_facts(target: &mut Vec<RetrievedFact>, facts: Vec<RetrievedFact>) {
        for fact in facts {
            if !target.iter().any(|existing| existing.same_identity(&fact)) {
                target.push(fact);
            }
        }
    }

    fn merge_entity_facts(
        context: &mut RetrievedContext,
        entity_name: &str,
        entity_type: &str,
        facts: Vec<RetrievedFact>,
    ) {
        if let Some(existing) = context
            .entities
            .iter_mut()
            .find(|entity| entity.name == entity_name && entity.entity_type == entity_type)
        {
            Self::merge_facts(&mut existing.facts, facts);
            return;
        }

        if entity_type != "Unknown" {
            if let Some(existing) = context
                .entities
                .iter_mut()
                .find(|entity| entity.name == entity_name && entity.entity_type == "Unknown")
            {
                existing.entity_type = entity_type.to_string();
                Self::merge_facts(&mut existing.facts, facts);
                return;
            }
        }

        if entity_type == "Unknown"
            && context
                .entities
                .iter()
                .any(|entity| entity.name == entity_name)
        {
            return;
        }

        context.entities.push(RetrievedEntity {
            name: entity_name.to_string(),
            entity_type: entity_type.to_string(),
            facts,
        });
    }

    fn accumulate_kg_related(result: &Value, context: &mut RetrievedContext) {
        let root_entity = result
            .get("root_entity")
            .and_then(Value::as_str)
            .unwrap_or("Unknown");
        let edges = result
            .get("edges")
            .and_then(Value::as_array)
            .map(|edges| {
                edges
                    .iter()
                    .filter_map(|edge| {
                        Some(RetrievedRelation {
                            subject_name: edge.get("subject")?.as_str()?.to_string(),
                            predicate: edge.get("predicate")?.as_str()?.to_string(),
                            object_name: edge.get("object")?.as_str()?.to_string(),
                            depth: edge.get("depth").and_then(Value::as_u64).unwrap_or(0) as u32,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        for edge in edges {
            if !context.relations.iter().any(|existing| {
                existing.subject_name == edge.subject_name
                    && existing.predicate == edge.predicate
                    && existing.object_name == edge.object_name
            }) {
                context.relations.push(edge);
            }
        }
        Self::merge_entity_facts(context, root_entity, "Unknown", Vec::new());
    }

    fn accumulate_kg_search(result: &Value, context: &mut RetrievedContext) {
        for match_result in result
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(entity) = match_result.get("entity") else {
                continue;
            };
            let entity_name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            let entity_type = entity
                .get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            let facts = match_result
                .get("top_facts")
                .and_then(Value::as_array)
                .map(|facts| {
                    facts
                        .iter()
                        .filter_map(Self::parse_retrieved_fact)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Self::merge_entity_facts(context, entity_name, entity_type, facts);
        }
    }

    fn accumulate_conversation(result: &Value, context: &mut RetrievedContext) {
        for snippet_value in result.as_array().into_iter().flatten() {
            let (Some(session_id), Some(role), Some(snippet)) = (
                snippet_value.get("session_id").and_then(Value::as_i64),
                snippet_value.get("role").and_then(Value::as_str),
                snippet_value.get("snippet").and_then(Value::as_str),
            ) else {
                continue;
            };
            let snippet = ConversationSnippet {
                session_id,
                role: role.to_string(),
                snippet: snippet.to_string(),
                created_at: snippet_value
                    .get("created_at")
                    .and_then(parse_utc)
                    .unwrap_or_else(Utc::now),
            };
            if !context.conversation_snippets.iter().any(|existing| {
                existing.session_id == snippet.session_id
                    && existing.role == snippet.role
                    && existing.snippet == snippet.snippet
            }) {
                context.conversation_snippets.push(snippet);
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RetrievalAgentError {
    #[error("retrieval task must not be empty")]
    EmptyTask,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn plan_initial_steps_deduplicates_tokens() {
        let steps = RetrievalAgent::plan_initial_steps("Mary mary's Mary");
        assert_eq!(
            steps,
            vec![
                RetrievalStep::KgSearch {
                    query: "Mary".to_string(),
                },
                RetrievalStep::ConversationSearch {
                    query: "Mary mary's Mary".to_string(),
                },
            ]
        );
    }

    #[test]
    fn plan_initial_steps_deduplicates_unicode_tokens() {
        let steps = RetrievalAgent::plan_initial_steps("Café café");
        assert_eq!(steps.len(), 2);
        assert!(matches!(steps[0], RetrievalStep::KgSearch { .. }));
    }

    fn fact(predicate: &str, confidence: f32) -> RetrievedFact {
        RetrievedFact {
            predicate: predicate.to_string(),
            object_name: None,
            object_literal: None,
            confidence,
            valid_from: None,
            valid_until: None,
            status: "active".to_string(),
            inferred: false,
        }
    }

    #[test]
    fn merge_entity_facts_upgrades_unknown_placeholder() {
        let mut context = RetrievedContext::default();
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Unknown", vec![]);
        assert_eq!(context.entities.len(), 1);
        assert_eq!(context.entities[0].entity_type, "Unknown");

        let mut fact = fact("allergic_to", 0.95);
        fact.object_literal = Some("shellfish".to_string());
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Person", vec![fact.clone()]);
        assert_eq!(context.entities.len(), 1);
        assert_eq!(context.entities[0].entity_type, "Person");
        assert_eq!(context.entities[0].facts, vec![fact]);
    }

    #[test]
    fn merge_entity_facts_skips_unknown_when_typed_exists() {
        let mut context = RetrievedContext::default();
        let mut fact = fact("allergic_to", 0.95);
        fact.object_literal = Some("shellfish".to_string());
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Person", vec![fact.clone()]);
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Unknown", vec![]);
        assert_eq!(context.entities.len(), 1);
        assert_eq!(context.entities[0].entity_type, "Person");
        assert_eq!(context.entities[0].facts, vec![fact]);
    }

    #[test]
    fn accumulate_kg_search_preserves_temporal_bounds() {
        let mut context = RetrievedContext::default();
        let result = serde_json::json!({
            "query": "appointments",
            "results": [{
                "entity": {"id": 1, "name": "Devansh", "entity_type": "Person"},
                "match_score": 1.0,
                "top_facts": [{
                    "predicate": "has_event",
                    "object_name": null,
                    "object_literal": "Property Check-In",
                    "confidence": 0.9,
                    "valid_from": "2025-07-16T00:00:00Z",
                    "valid_until": "2025-07-20T00:00:00Z"
                }]
            }]
        });
        RetrievalAgent::accumulate_kg_search(&result, &mut context);
        assert_eq!(
            context.entities[0].facts[0].valid_from,
            Some(chrono::Utc.with_ymd_and_hms(2025, 7, 16, 0, 0, 0).unwrap())
        );
        assert_eq!(
            context.entities[0].facts[0].valid_until,
            Some(chrono::Utc.with_ymd_and_hms(2025, 7, 20, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn accumulate_conversation_deduplicates_by_session_role_and_snippet() {
        let mut context = RetrievedContext::default();
        let result = serde_json::json!([{
            "session_id": 1,
            "role": "user",
            "snippet": "Mary likes shellfish",
            "created_at": "2026-01-01T00:00:00Z"
        }]);

        RetrievalAgent::accumulate_conversation(&result, &mut context);
        RetrievalAgent::accumulate_conversation(&result, &mut context);

        assert_eq!(context.conversation_snippets.len(), 1);
    }
}
