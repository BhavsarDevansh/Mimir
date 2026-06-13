//! Data types for agentic context retrieval.
//!
//! `RetrievedContext` is the structured output of the RetrievalAgent,
//! consumed by the main LLM after the internal research phase completes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Root container returned by the RetrievalAgent after investigating
/// the knowledge graph and conversation history.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RetrievedContext {
    /// Entities whose facts were gathered, keyed by entity name.
    pub entities: Vec<RetrievedEntity>,
    /// Directed relations discovered via graph traversal.
    pub relations: Vec<RetrievedRelation>,
    /// Relevant snippets from past conversations.
    pub conversation_snippets: Vec<ConversationSnippet>,
    /// Optional high-level summary of why the agent finished.
    pub finish_reason: Option<String>,
    /// Number of internal tool-call rounds consumed.
    pub rounds_used: u16,
}

impl RetrievedContext {
    /// Human-readable summary for display.
    pub fn summary(&self) -> String {
        format!(
            "Retrieved {} facts across {} entities, {} relations, and {} conversation snippets",
            self.entities.iter().map(|e| e.facts.len()).sum::<usize>(),
            self.entities.len(),
            self.relations.len(),
            self.conversation_snippets.len(),
        )
    }
}

/// An entity discovered during retrieval with its collected facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievedEntity {
    pub name: String,
    pub entity_type: String,
    pub facts: Vec<RetrievedFact>,
}

/// A single fact extracted from a `kg_query` or `kg_related` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievedFact {
    pub predicate: String,
    pub object_name: Option<String>,
    pub object_literal: Option<String>,
    pub confidence: f32,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub status: String,
    pub inferred: bool,
}

impl RetrievedFact {
    /// Identity key for deduplication inside a single entity.
    ///
    /// Two facts are considered the same record when every structural and
    /// lifecycle field matches. This prevents distinct revisions, temporal
    /// ranges, or lifecycle states from being collapsed during retrieval.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.predicate == other.predicate
            && self.object_name == other.object_name
            && self.object_literal == other.object_literal
            && self.confidence.to_bits() == other.confidence.to_bits()
            && self.valid_from == other.valid_from
            && self.valid_until == other.valid_until
            && self.status == other.status
            && self.inferred == other.inferred
    }
}

/// A directed edge from graph traversal (`kg_related`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievedRelation {
    pub subject_name: String,
    pub predicate: String,
    pub object_name: String,
    pub depth: u32,
}

/// A snippet from conversation history search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationSnippet {
    pub session_id: i64,
    pub role: String,
    pub snippet: String,
    pub created_at: DateTime<Utc>,
}
