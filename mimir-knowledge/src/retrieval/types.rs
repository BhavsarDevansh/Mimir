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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

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
    fn summary_counts_facts_entities_relations_and_snippets() {
        let ctx = RetrievedContext {
            entities: vec![
                RetrievedEntity {
                    name: "Alice".to_string(),
                    entity_type: "person".to_string(),
                    facts: vec![fact("lives_in", 0.9), fact("works_as", 0.8)],
                },
                RetrievedEntity {
                    name: "Bob".to_string(),
                    entity_type: "person".to_string(),
                    facts: vec![fact("knows", 0.5)],
                },
            ],
            relations: vec![RetrievedRelation {
                subject_name: "Alice".to_string(),
                predicate: "knows".to_string(),
                object_name: "Bob".to_string(),
                depth: 1,
            }],
            conversation_snippets: vec![ConversationSnippet {
                session_id: 1,
                role: "user".to_string(),
                snippet: "hi".to_string(),
                created_at: Utc::now(),
            }],
            finish_reason: None,
            rounds_used: 3,
        };
        assert_eq!(
            ctx.summary(),
            "Retrieved 3 facts across 2 entities, 1 relations, and 1 conversation snippets"
        );
    }

    #[test]
    fn summary_empty_context_reports_zeros() {
        let ctx = RetrievedContext::default();
        assert_eq!(
            ctx.summary(),
            "Retrieved 0 facts across 0 entities, 0 relations, and 0 conversation snippets"
        );
    }

    #[test]
    fn same_identity_true_for_equal_facts() {
        let a = fact("lives_in", 0.9);
        assert!(a.same_identity(&a));
    }

    #[test]
    fn same_identity_false_when_predicate_differs() {
        let a = fact("lives_in", 0.9);
        let mut b = a.clone();
        b.predicate = "works_at".to_string();
        assert!(!a.same_identity(&b));
    }

    #[test]
    fn same_identity_false_when_confidence_differs() {
        let a = fact("lives_in", 0.9);
        let b = fact("lives_in", 0.8);
        assert!(!a.same_identity(&b));
    }

    #[test]
    fn same_identity_uses_bit_pattern_for_confidence() {
        // +0.0 and -0.0 are == as f32 but differ in bit pattern; same_identity
        // compares bits, so they must be treated as distinct.
        let a = fact("x", 0.0_f32);
        let mut b = a.clone();
        b.confidence = -0.0_f32;
        assert!(!a.same_identity(&b));
    }

    #[test]
    fn same_identity_false_when_status_or_inferred_differs() {
        let a = fact("x", 0.5);
        let mut b = a.clone();
        b.status = "dormant".to_string();
        assert!(!a.same_identity(&b));
        let mut c = a.clone();
        c.inferred = true;
        assert!(!a.same_identity(&c));
    }

    #[test]
    fn same_identity_false_when_temporal_windows_differ() {
        let a = fact("x", 0.5);
        let mut b = a.clone();
        b.valid_from = Some(Utc::now());
        assert!(!a.same_identity(&b));
    }

    #[test]
    fn retrieved_context_serde_roundtrip() {
        let ctx = RetrievedContext {
            entities: vec![RetrievedEntity {
                name: "Alice".to_string(),
                entity_type: "person".to_string(),
                facts: vec![fact("lives_in", 0.9)],
            }],
            relations: vec![],
            conversation_snippets: vec![],
            finish_reason: Some("done".to_string()),
            rounds_used: 2,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: RetrievedContext = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ctx);
    }
}
