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

/// Upper bounds that keep a single retrieval task from fanning out into an
/// unbounded number of concurrent database queries and allocations. They keep
/// performance and security central: an adversarial or accidental long task
/// degrades into a bounded plan instead of thousands of concurrent lookups.
/// Maximum characters of the task considered during planning.
const MAX_TASK_CHARS: usize = 2_000;
/// Maximum distinct `kg_search` steps planned for one retrieval task.
const MAX_TOKEN_STEPS: usize = 16;
/// Maximum distinct candidate entities that receive `kg_query`/`kg_related`
/// follow-up steps.
const MAX_CANDIDATES: usize = 12;
/// Maximum salient tokens OR-joined into the conversation query.
const MAX_SALIENT_TOKENS: usize = 8;
/// Maximum retrieval steps executed concurrently.
const MAX_CONCURRENCY: usize = 8;
/// Facts requested per `kg_query` page (the tool clamps `limit` to 50).
const KG_QUERY_PAGE_SIZE: i64 = 50;
/// Maximum `kg_query` pages fetched per candidate, bounding facts per
/// candidate at `KG_QUERY_PAGE_SIZE * MAX_KG_QUERY_PAGES`.
const MAX_KG_QUERY_PAGES: usize = 4;

/// Filler words excluded from the salient-term conversation query. They
/// rarely identify conversation evidence on their own and would otherwise
/// dominate relaxed OR matching.
const STOP_WORDS: &[&str] = &[
    "a", "all", "also", "an", "and", "any", "are", "as", "at", "be", "been", "but", "by", "can",
    "could", "did", "do", "does", "find", "for", "from", "get", "give", "had", "has", "have",
    "her", "him", "his", "how", "i", "if", "in", "into", "is", "it", "its", "just", "know", "look",
    "may", "me", "might", "must", "my", "need", "no", "not", "now", "of", "on", "or", "our", "out",
    "over", "please", "shall", "she", "so", "some", "tell", "than", "that", "the", "their", "them",
    "then", "there", "these", "they", "this", "those", "to", "too", "under", "up", "was", "we",
    "were", "what", "when", "where", "which", "who", "whom", "why", "will", "with", "would", "you",
    "your",
];

/// Replace case-insensitive possessive suffixes ('s, ’s) with a space
/// so they never survive tokenisation as a junk `S` search token, regardless
/// of the surrounding letter casing (`JAMES'S`, `James’s`, ...).
fn strip_possessives(task: &str) -> String {
    let mut out = String::with_capacity(task.len());
    let mut characters = task.chars().peekable();
    while let Some(character) = characters.next() {
        if (character == '\'' || character == '\u{2019}')
            && matches!(characters.peek(), Some('s') | Some('S'))
        {
            characters.next();
            out.push(' ');
        } else {
            out.push(character);
        }
    }
    out
}

/// Extract distinct salient terms for the relaxed conversation query:
/// tokens of at least two characters that are not filler words, deduplicated
/// case-insensitively and capped.
fn salient_tokens(task: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in task.split(|character: char| !character.is_alphanumeric()) {
        if token.chars().count() < 2 {
            continue;
        }
        let lowercased: String = token.chars().flat_map(char::to_lowercase).collect();
        if STOP_WORDS.contains(&lowercased.as_str()) {
            continue;
        }
        if tokens.iter().any(|existing| same_text(existing, token)) {
            continue;
        }
        tokens.push(token.to_string());
        if tokens.len() >= MAX_SALIENT_TOKENS {
            break;
        }
    }
    tokens
}

#[derive(Debug, Clone, PartialEq)]
enum RetrievalStep {
    KgSearch { query: String },
    ConversationSearch { query: String, match_any: bool },
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
        if task.len() > MAX_TASK_CHARS {
            warn!(
                length = task.len(),
                cap = MAX_TASK_CHARS,
                "truncating retrieval task for planning"
            );
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

        // The planning caps bound the plan at
        // MAX_TOKEN_STEPS + 1 + 2 * MAX_CANDIDATES steps, far below u16::MAX.
        context.steps_executed =
            u16::try_from(initial_steps.len() + follow_up_steps.len()).unwrap_or(u16::MAX);
        context.finish_reason = Some("completed".to_string());
        info!(
            steps = context.steps_executed,
            "deterministic retrieval completed"
        );
        Ok(context)
    }

    /// Build the initial fixed plan: one entity search per distinct task
    /// token (capped) and one salient-term conversation search.
    fn plan_initial_steps(task: &str) -> Vec<RetrievalStep> {
        let task = Self::truncate_task(task);
        let task_without_possessives = strip_possessives(task);
        let mut steps = Vec::new();
        for token in task_without_possessives.split(|character: char| !character.is_alphanumeric())
        {
            if token.is_empty()
                || steps.len() >= MAX_TOKEN_STEPS
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
        // Relaxed OR matching over salient terms keeps conversation evidence
        // reachable for ordinary natural-language tasks, which an all-token
        // AND query would reject unless a stored message contains every
        // filler word too. Tasks without any salient token fall back to the
        // full task under the same relaxed mode.
        let salient = salient_tokens(&task_without_possessives);
        let query = if salient.is_empty() {
            task.to_string()
        } else {
            salient.join(" OR ")
        };
        steps.push(RetrievalStep::ConversationSearch {
            query,
            match_any: true,
        });
        steps
    }

    /// Cap the task text consulted during planning so tokenisation and the
    /// dedupe scans stay bounded regardless of the incoming task length.
    fn truncate_task(task: &str) -> &str {
        if task.len() <= MAX_TASK_CHARS {
            return task;
        }
        let mut end = MAX_TASK_CHARS;
        while end > 0 && !task.is_char_boundary(end) {
            end -= 1;
        }
        &task[..end]
    }

    /// Query facts and relationships once for every distinct search
    /// candidate, capped so a wide search cannot fan out unbounded.
    fn plan_follow_up_steps(candidates: &[String]) -> Vec<RetrievalStep> {
        if candidates.len() > MAX_CANDIDATES {
            warn!(
                candidates = candidates.len(),
                cap = MAX_CANDIDATES,
                "capping retrieval follow-up candidates"
            );
        }
        candidates
            .iter()
            .take(MAX_CANDIDATES)
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

    /// Execute every step with bounded concurrency. Tool failures are logged
    /// and omitted from accumulated context, but never prevent other steps
    /// from running.
    async fn execute_steps(
        &self,
        steps: &[RetrievalStep],
    ) -> Vec<(&'static str, Result<ToolOutput, ToolError>)> {
        // Fixed-size chunks keep at most `MAX_CONCURRENCY` futures in flight,
        // so a long plan cannot launch every database query at once, while
        // `join_all` per chunk keeps results aligned with their steps.
        let mut results = Vec::with_capacity(steps.len());
        for chunk in steps.chunks(MAX_CONCURRENCY) {
            let futures = chunk.iter().map(|step| self.execute_step(step));
            results.extend(join_all(futures).await);
        }
        debug!(
            steps = steps.len(),
            concurrent = MAX_CONCURRENCY,
            "executed retrieval steps"
        );
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
            RetrievalStep::ConversationSearch { query, match_any } => {
                self.conversation_search
                    .execute(serde_json::json!({
                        "query": query,
                        "limit": 20,
                        "match_any": match_any
                    }))
                    .await
            }
            RetrievalStep::KgQuery { entity_name } => {
                Self::execute_kg_query(&self.kg_query, entity_name).await
            }
            RetrievalStep::KgRelated { entity_name } => {
                self.kg_related
                    .execute(serde_json::json!({"entity_name": entity_name}))
                    .await
            }
        }
    }

    /// Fetch a candidate's facts with pagination so nothing is silently
    /// truncated, bounded by `MAX_KG_QUERY_PAGES` pages of
    /// `KG_QUERY_PAGE_SIZE`. When facts remain beyond the final page the
    /// merged output carries `"truncated": true` and the truncation is
    /// logged, surfacing the deterministic cap instead of hiding it.
    async fn execute_kg_query(
        kg_query: &KgQueryTool,
        entity_name: &str,
    ) -> Result<ToolOutput, ToolError> {
        let mut facts: Vec<Value> = Vec::new();
        let mut entity: Option<Value> = None;
        let mut total = 0usize;
        let mut truncated = false;
        for page in 0..MAX_KG_QUERY_PAGES {
            let offset = (page as i64) * KG_QUERY_PAGE_SIZE;
            let output = match kg_query
                .execute(serde_json::json!({
                    "entity_name": entity_name,
                    "offset": offset,
                    "limit": KG_QUERY_PAGE_SIZE
                }))
                .await
            {
                Ok(output) => output,
                Err(error) => {
                    if facts.is_empty() {
                        return Err(error);
                    }
                    warn!(
                        entity = entity_name,
                        fetched = facts.len(),
                        "kg_query pagination stopped early after tool failure"
                    );
                    truncated = true;
                    break;
                }
            };
            let Some(value) = output.result else {
                if facts.is_empty() {
                    return Ok(output);
                }
                break;
            };
            if entity.is_none() {
                entity = value.get("entity").cloned();
            }
            if let Some(page_facts) = value.get("facts").and_then(Value::as_array) {
                facts.extend(page_facts.iter().cloned());
            }
            total = value
                .get("total")
                .and_then(Value::as_u64)
                .map(|t| t as usize)
                .unwrap_or(total);
            if facts.len() >= total {
                break;
            }
        }
        if facts.len() < total {
            truncated = true;
            warn!(
                entity = entity_name,
                total = total,
                fetched = facts.len(),
                "kg_query facts deterministically truncated"
            );
        }
        let result = serde_json::json!({
            "entity": entity.unwrap_or(Value::Null),
            "facts": facts,
            "total": total,
            "offset": 0,
            "limit": facts.len() as i64,
            "truncated": truncated
        });
        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
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
                    query: "Mary".to_string(),
                    match_any: true,
                },
            ]
        );
    }

    #[test]
    fn plan_initial_steps_strips_uppercase_possessives() {
        // Both quote styles must be stripped regardless of letter casing so
        // no junk `S` token is planned.
        let steps = RetrievalAgent::plan_initial_steps("JAMES'S JAMES\u{2019}S");
        assert_eq!(steps.len(), 2);
        assert!(
            steps.iter().any(|step| {
                matches!(step, RetrievalStep::KgSearch { query } if query == "JAMES")
            })
        );
        assert!(steps.iter().all(|step| !matches!(
            step,
            RetrievalStep::KgSearch { query } if same_text(query, "s")
        )));
    }

    #[test]
    fn plan_initial_steps_caps_token_steps() {
        let task = (0..40)
            .map(|i| format!("entity{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let steps = RetrievalAgent::plan_initial_steps(&task);
        // One `kg_search` per distinct token up to the cap, plus one
        // conversation search.
        assert_eq!(steps.len(), MAX_TOKEN_STEPS + 1);
        assert!(matches!(
            steps.last(),
            Some(RetrievalStep::ConversationSearch { .. })
        ));
    }

    #[test]
    fn plan_initial_steps_conversation_query_relaxes_to_or() {
        // An ordinary natural-language task must not become one all-token
        // AND query; only salient terms are OR-joined so filler words
        // cannot force the match.
        let steps =
            RetrievalAgent::plan_initial_steps("Find Mary's food preferences and any allergies");
        let (query, match_any) = match steps.last().unwrap() {
            RetrievalStep::ConversationSearch { query, match_any } => (query.clone(), match_any),
            _ => panic!("expected conversation step"),
        };
        assert!(match_any);
        assert_eq!(query, "Mary OR food OR preferences OR allergies");
    }

    #[test]
    fn plan_initial_steps_conversation_caps_salient_tokens() {
        let task = (0..30)
            .map(|i| format!("topic{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let query = match steps_last(&task) {
            RetrievalStep::ConversationSearch { query, .. } => query.clone(),
            _ => panic!("expected conversation step"),
        };
        assert_eq!(query.split(" OR ").count(), MAX_SALIENT_TOKENS);
    }

    #[test]
    fn plan_initial_steps_conversation_falls_back_to_full_task() {
        // Without any salient token the full task is searched, still under
        // relaxed OR matching so it cannot silently require every word.
        let (query, match_any) = match steps_last("a of the") {
            RetrievalStep::ConversationSearch { query, match_any } => (query.clone(), match_any),
            _ => panic!("expected conversation step"),
        };
        assert!(match_any);
        assert_eq!(query, "a of the");
    }

    #[test]
    fn plan_follow_up_steps_caps_candidates() {
        let candidates = (0..30).map(|i| format!("Entity{i}")).collect::<Vec<_>>();
        let steps = RetrievalAgent::plan_follow_up_steps(&candidates);
        assert_eq!(steps.len(), MAX_CANDIDATES * 2);
        assert!(steps.iter().all(|step| matches!(
            step,
            RetrievalStep::KgQuery { .. } | RetrievalStep::KgRelated { .. }
        )));
    }

    #[test]
    fn truncate_task_caps_and_keeps_char_boundaries() {
        assert_eq!(RetrievalAgent::truncate_task("short"), "short");
        let long = "\u{e9}".repeat(MAX_TASK_CHARS); // 2-byte characters
        let truncated = RetrievalAgent::truncate_task(&long);
        assert!(truncated.len() <= MAX_TASK_CHARS);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(long.starts_with(truncated));
    }

    #[test]
    fn strip_possessives_handles_both_quotes_and_cases() {
        assert_eq!(strip_possessives("James's car"), "James  car");
        assert_eq!(strip_possessives("JAMES\u{2019}S car"), "JAMES  car");
        assert_eq!(strip_possessives("it's-its"), "it -its");
        assert_eq!(strip_possessives("class of 1999"), "class of 1999");
    }

    /// Last planned step (always the conversation search).
    fn steps_last(task: &str) -> RetrievalStep {
        let steps = RetrievalAgent::plan_initial_steps(task);
        steps.into_iter().last().unwrap()
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
