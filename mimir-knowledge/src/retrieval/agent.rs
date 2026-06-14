//! RetrievalAgent — ephemeral LLM session for multi-step KB + conversation investigation.

use std::sync::Arc;

use async_trait::async_trait;
use mimir_core::llm::backend::LlmBackend;
use mimir_core::llm::types::Message;
use mimir_core::tools::{Tool, ToolError, ToolOutput, ToolPermission, ToolRegistry};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::KnowledgeGraph;
use crate::retrieval::types::{
    ConversationSnippet, RetrievedContext, RetrievedEntity, RetrievedFact, RetrievedRelation,
};
use crate::tools::{KgQueryTool, KgRelatedTool, KgSearchTool};

/// Internal agent that investigates the knowledge graph and conversation history.
///
/// Lives for a single retrieval task.  Maintains its own ephemeral message history
/// and a private tool registry containing only retrieval tools.
pub struct RetrievalAgent {
    llm: Arc<dyn LlmBackend>,
    private_registry: ToolRegistry,
}

impl RetrievalAgent {
    /// Maximum internal tool-call rounds before hard termination.
    pub const MAX_ROUNDS: u16 = 25;

    /// Create a new agent backed by `llm` and the shared knowledge graph / context manager.
    pub fn new(
        llm: Arc<dyn LlmBackend>,
        kg: Arc<KnowledgeGraph>,
        context_manager: Arc<mimir_core::context::ContextManager>,
    ) -> Self {
        let private_registry = ToolRegistry::new();

        // Retrieval-only tools.
        let _ = private_registry.register_native(Arc::new(KgQueryTool::new(Arc::clone(&kg))));
        let _ = private_registry.register_native(Arc::new(KgRelatedTool::new(Arc::clone(&kg))));
        let _ = private_registry.register_native(Arc::new(KgSearchTool::new(Arc::clone(&kg))));
        let _ = private_registry.register_native(Arc::new(
            mimir_core::tools::SearchConversationHistoryTool::new(Arc::clone(&context_manager)),
        ));
        let _ = private_registry.register_native(Arc::new(mimir_core::tools::GetCurrentTimeTool));

        // Internal termination signal.
        let _ = private_registry.register_native(Arc::new(FinishRetrievalTool));

        Self {
            llm,
            private_registry,
        }
    }

    /// Run the retrieval investigation for `task`.
    ///
    /// Returns the accumulated `RetrievedContext` when the agent calls
    /// `finish_retrieval` or when `MAX_ROUNDS` is reached.
    pub async fn retrieve(&self, task: &str) -> Result<RetrievedContext, RetrievalAgentError> {
        let system_prompt = format!(
            "You are Mimir's research subsystem. \n\n
            Your ONLY job is to thoroughly investigate the following request using the knowledge graph \
            and conversation history. Query specific entities, traverse relationships, and search past \
            conversations until you are confident you have all relevant context. \n
            When you are satisfied, call `finish_retrieval`. Do not call it alongside other tools. \n
            Request: {}",
            task
        );

        let mut conversation = vec![
            Message::system(system_prompt),
            Message::user("Please begin your investigation."),
        ];

        let tools_opt = self.private_registry.export_openai_tools_for_llm();
        let mut context = RetrievedContext::default();

        for round in 0..Self::MAX_ROUNDS {
            debug!(round, "retrieval agent round");

            let (assistant_msg, _usage) = self
                .llm
                .chat_message(conversation.clone(), tools_opt.clone())
                .await
                .map_err(RetrievalAgentError::Llm)?;

            let tool_calls = match assistant_msg.tool_calls {
                Some(calls) if !calls.is_empty() => calls,
                _ => {
                    // No tool calls — agent finished without finish_retrieval.
                    // Return whatever we have.
                    warn!(
                        round,
                        "retrieval agent produced no tool calls; terminating early"
                    );
                    context.rounds_used = round + 1;
                    return Ok(context);
                }
            };

            // Track whether finish_retrieval was the sole tool call.
            let mut finished = false;
            let mut other_tools = Vec::new();

            for tc in &tool_calls {
                if tc.function.name == "finish_retrieval" {
                    finished = true;
                } else {
                    other_tools.push(tc);
                }
            }

            if finished && other_tools.is_empty() {
                info!(round, "retrieval agent finished via finish_retrieval");
                context.rounds_used = round + 1;
                // Extract optional reason from finish_retrieval arguments.
                if let Some(finish_tc) = tool_calls
                    .iter()
                    .find(|tc| tc.function.name == "finish_retrieval")
                {
                    if let Ok(args) =
                        serde_json::from_str::<serde_json::Value>(&finish_tc.function.arguments)
                    {
                        context.finish_reason = args
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                    }
                }
                return Ok(context);
            }

            // Accumulate tool outputs into the context.
            let mut tool_result_msgs = Vec::new();
            for tc in &tool_calls {
                if tc.function.name == "finish_retrieval" {
                    tool_result_msgs.push(Message::tool(
                        &tc.id,
                        "Ignored: finish_retrieval must be called alone, not alongside other tools.",
                    ));
                    continue;
                }

                let args =
                    serde_json::from_str::<Value>(&tc.function.arguments).unwrap_or(Value::Null);
                let output = match self.private_registry.execute(&tc.function.name, args).await {
                    Ok(out) => out,
                    Err(e) => {
                        warn!(tool = %tc.function.name, "retrieval tool failed: {}", e);
                        tool_result_msgs.push(Message::tool(&tc.id, format!("Tool error: {}", e)));
                        continue;
                    }
                };

                // Parse and accumulate.
                if let Some(ref result) = output.result {
                    self.accumulate_result(&tc.function.name, result, &mut context);
                }

                tool_result_msgs.push(Message::tool(&tc.id, output.to_llm_text()));
            }

            // Push assistant message + tool results back into conversation.
            conversation.push(Message {
                role: "assistant".to_string(),
                content: assistant_msg.content,
                tool_calls: Some(tool_calls),
                tool_call_id: None,
            });
            conversation.extend(tool_result_msgs);
        }

        // Max rounds reached.
        warn!(
            "retrieval agent hit MAX_ROUNDS ({}) without finish_retrieval",
            Self::MAX_ROUNDS
        );
        context.rounds_used = Self::MAX_ROUNDS;
        Ok(context)
    }

    // ------------------------------------------------------------------
    // Accumulation helpers
    // ------------------------------------------------------------------

    fn accumulate_result(&self, tool_name: &str, result: &Value, context: &mut RetrievedContext) {
        match tool_name {
            "kg_query" => self.accumulate_kg_query(result, context),
            "kg_related" => self.accumulate_kg_related(result, context),
            "kg_search" => self.accumulate_kg_search(result, context),
            "search_conversation_history" => self.accumulate_conversation(result, context),
            _ => {}
        }
    }

    fn accumulate_kg_query(&self, result: &Value, context: &mut RetrievedContext) {
        let entity_name = result
            .get("entity")
            .and_then(|e| e.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let entity_type = result
            .get("entity")
            .and_then(|e| e.get("entity_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");

        let facts = result
            .get("facts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| {
                        Some(RetrievedFact {
                            predicate: f.get("predicate")?.as_str()?.to_string(),
                            object_name: f
                                .get("object_name")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            object_literal: f
                                .get("object_literal")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            confidence: f.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0)
                                as f32,
                            valid_from: f.get("valid_from").and_then(|v| {
                                chrono::DateTime::parse_from_rfc3339(v.as_str()?)
                                    .ok()
                                    .map(|dt| dt.with_timezone(&chrono::Utc))
                            }),
                            valid_until: f.get("valid_until").and_then(|v| {
                                chrono::DateTime::parse_from_rfc3339(v.as_str()?)
                                    .ok()
                                    .map(|dt| dt.with_timezone(&chrono::Utc))
                            }),
                            status: f
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Active")
                                .to_string(),
                            inferred: f.get("inferred").and_then(|v| v.as_bool()).unwrap_or(false),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Self::merge_entity_facts(context, entity_name, entity_type, facts);
    }

    /// Merge a set of facts into an entity, creating the entity if needed.
    ///
    /// Merge `facts` into `target`, deduplicating by full fact identity.
    fn merge_facts(target: &mut Vec<RetrievedFact>, facts: Vec<RetrievedFact>) {
        for fact in facts {
            if !target.iter().any(|f| f.same_identity(&fact)) {
                target.push(fact);
            }
        }
    }

    /// Deduplication uses the full identity of both the entity (`name` +
    /// `entity_type`) and each fact (all structural and lifecycle fields) so
    /// that homonymous entities and distinct fact revisions are preserved.
    ///
    /// When a typed entity is merged and an "Unknown" placeholder with the same
    /// name already exists (e.g. from `kg_related` root-entity accumulation), the
    /// placeholder is upgraded in place rather than creating a duplicate entry.
    /// Conversely, an "Unknown" placeholder is never added if a typed entity
    /// with the same name is already present.
    fn merge_entity_facts(
        context: &mut RetrievedContext,
        entity_name: &str,
        entity_type: &str,
        facts: Vec<RetrievedFact>,
    ) {
        // Exact (name, type) match: merge facts only.
        if let Some(existing) = context
            .entities
            .iter_mut()
            .find(|e| e.name == entity_name && e.entity_type == entity_type)
        {
            Self::merge_facts(&mut existing.facts, facts);
            return;
        }

        // Typed entity colliding with an Unknown placeholder: upgrade the placeholder.
        if entity_type != "Unknown" {
            if let Some(existing) = context
                .entities
                .iter_mut()
                .find(|e| e.name == entity_name && e.entity_type == "Unknown")
            {
                existing.entity_type = entity_type.to_string();
                Self::merge_facts(&mut existing.facts, facts);
                return;
            }
        }

        // Unknown placeholder colliding with a typed entity: skip to avoid a duplicate.
        if entity_type == "Unknown" && context.entities.iter().any(|e| e.name == entity_name) {
            return;
        }

        context.entities.push(RetrievedEntity {
            name: entity_name.to_string(),
            entity_type: entity_type.to_string(),
            facts,
        });
    }

    fn accumulate_kg_related(&self, result: &Value, context: &mut RetrievedContext) {
        let root_entity = result
            .get("root_entity")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");

        let edges = result
            .get("edges")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(RetrievedRelation {
                            subject_name: e.get("subject")?.as_str()?.to_string(),
                            predicate: e.get("predicate")?.as_str()?.to_string(),
                            object_name: e.get("object")?.as_str()?.to_string(),
                            depth: e.get("depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        for edge in edges {
            if !context.relations.iter().any(|r| {
                r.subject_name == edge.subject_name
                    && r.predicate == edge.predicate
                    && r.object_name == edge.object_name
            }) {
                context.relations.push(edge);
            }
        }

        // Also accumulate root entity if not already present (empty facts list).
        Self::merge_entity_facts(context, root_entity, "Unknown", Vec::new());
    }

    fn accumulate_kg_search(&self, result: &Value, context: &mut RetrievedContext) {
        let empty_vec: Vec<Value> = Vec::new();
        let results = result
            .get("results")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty_vec);

        for r in results {
            let entity_name = r
                .get("entity")
                .and_then(|e| e.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            let entity_type = r
                .get("entity")
                .and_then(|e| e.get("entity_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");

            let facts = r
                .get("top_facts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| {
                            Some(RetrievedFact {
                                predicate: f.get("predicate")?.as_str()?.to_string(),
                                object_name: f
                                    .get("object_name")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                object_literal: f
                                    .get("object_literal")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                confidence: f
                                    .get("confidence")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0)
                                    as f32,
                                valid_from: None,
                                valid_until: None,
                                status: "Active".to_string(),
                                inferred: false,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Self::merge_entity_facts(context, entity_name, entity_type, facts);
        }
    }

    fn accumulate_conversation(&self, result: &Value, context: &mut RetrievedContext) {
        let empty_vec: Vec<Value> = Vec::new();
        let snippets = result.as_array().unwrap_or(&empty_vec);

        for s in snippets {
            let snippet = match (
                s.get("session_id").and_then(|v| v.as_i64()),
                s.get("role").and_then(|v| v.as_str()),
                s.get("snippet").and_then(|v| v.as_str()),
            ) {
                (Some(session_id), Some(role), Some(text)) => ConversationSnippet {
                    session_id,
                    role: role.to_string(),
                    snippet: text.to_string(),
                    created_at: s
                        .get("created_at")
                        .and_then(|v| {
                            chrono::DateTime::parse_from_rfc3339(v.as_str()?)
                                .ok()
                                .map(|dt| dt.with_timezone(&chrono::Utc))
                        })
                        .unwrap_or_else(chrono::Utc::now),
                },
                _ => continue,
            };

            if !context.conversation_snippets.iter().any(|existing| {
                existing.session_id == snippet.session_id && existing.snippet == snippet.snippet
            }) {
                context.conversation_snippets.push(snippet);
            }
        }
    }
}

// ------------------------------------------------------------------
// Errors
// ------------------------------------------------------------------

/// Errors specific to the retrieval agent.
#[derive(Debug, thiserror::Error)]
pub enum RetrievalAgentError {
    #[error("LLM backend error: {0}")]
    Llm(mimir_core::llm::types::LlmError),
}

// ------------------------------------------------------------------
// FinishRetrievalTool (internal termination signal)
// ------------------------------------------------------------------

struct FinishRetrievalTool;

#[async_trait]
impl Tool for FinishRetrievalTool {
    fn name(&self) -> &str {
        "finish_retrieval"
    }

    fn display_name(&self) -> &str {
        "Finish Retrieval"
    }

    fn description(&self) -> &str {
        "Signal that the investigation is complete and the agent should return the accumulated context."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Optional brief reason for concluding the investigation."
                }
            },
            "additionalProperties": false
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        // The agent intercepts this call before reaching here.
        // This fallback is a safety net.
        Ok(ToolOutput {
            result: Some(serde_json::json!({"status": "finished"})),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_entity_facts_upgrades_unknown_placeholder() {
        let mut context = RetrievedContext::default();

        // Root-entity accumulation adds an Unknown placeholder first.
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Unknown", vec![]);
        assert_eq!(context.entities.len(), 1);
        assert_eq!(context.entities[0].entity_type, "Unknown");

        // Later kg_query returns a typed entity with the same name.
        let fact = RetrievedFact {
            predicate: "allergic_to".to_string(),
            object_name: None,
            object_literal: Some("shellfish".to_string()),
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            status: "Active".to_string(),
            inferred: false,
        };
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Person", vec![fact.clone()]);

        // The placeholder should be upgraded, not duplicated.
        assert_eq!(context.entities.len(), 1);
        assert_eq!(context.entities[0].entity_type, "Person");
        assert_eq!(context.entities[0].facts, vec![fact]);
    }

    #[test]
    fn merge_entity_facts_skips_unknown_when_typed_exists() {
        let mut context = RetrievedContext::default();

        // Typed entity arrives first.
        let fact = RetrievedFact {
            predicate: "allergic_to".to_string(),
            object_name: None,
            object_literal: Some("shellfish".to_string()),
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            status: "Active".to_string(),
            inferred: false,
        };
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Person", vec![fact.clone()]);

        // Root-entity accumulation should not add an Unknown duplicate.
        RetrievalAgent::merge_entity_facts(&mut context, "Mary", "Unknown", vec![]);

        assert_eq!(context.entities.len(), 1);
        assert_eq!(context.entities[0].entity_type, "Person");
        assert_eq!(context.entities[0].facts, vec![fact]);
    }
}
