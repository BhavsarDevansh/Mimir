//! Librarian Agent — system-driven background fact extraction.
//!
//! The Librarian receives a completed [`ConversationTurn`] together with the
//! full KB snapshot (condensed memory, user identity, recent related facts)
//! and extracts/validates/stores structured facts.

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;

use mimir_core::agents::{Agent, AgentContext};
use mimir_core::conversation::ConversationTurn;
use mimir_core::identity::UserIdentity;
use mimir_core::llm::backend::LlmBackend;

use crate::KnowledgeGraph;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibrarianGoal {
    pub target_subject_id: i32,
    pub topic: String,
    pub turn: ConversationTurn,
}

impl LibrarianGoal {
    pub fn new(target_subject_id: i32, topic: impl Into<String>, turn: ConversationTurn) -> Self {
        Self {
            target_subject_id,
            topic: topic.into(),
            turn,
        }
    }
}

/// Runtime context passed to the LibrarianAgent by the runtime.
#[derive(Debug, Clone)]
pub struct LibrarianContext {
    pub knowledge_graph: Arc<KnowledgeGraph>,
    pub llm: Arc<dyn LlmBackend>,
    pub identity: UserIdentity,
    pub condensed_memory: Option<String>,
}

impl LibrarianContext {
    pub fn new(
        knowledge_graph: Arc<KnowledgeGraph>,
        llm: Arc<dyn LlmBackend>,
        identity: UserIdentity,
        condensed_memory: Option<String>,
    ) -> Self {
        Self {
            knowledge_graph,
            llm,
            identity,
            condensed_memory,
        }
    }
}

impl AgentContext for LibrarianContext {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Background agent that extracts structured facts from a completed conversation turn.
pub struct LibrarianAgent;

impl LibrarianAgent {
    /// Create a new LibrarianAgent.
    pub fn new() -> Self {
        Self
    }
}

impl Default for LibrarianAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Agent for LibrarianAgent {
    type Goal = LibrarianGoal;
    const KIND: &'static str = "librarian";

    async fn run(&self, goal: LibrarianGoal, ctx: Arc<dyn AgentContext>) -> anyhow::Result<()> {
        let ctx = ctx
            .as_any()
            .downcast_ref::<LibrarianContext>()
            .ok_or_else(|| anyhow::anyhow!("LibrarianAgent requires LibrarianContext"))?;

        let outcome = ctx
            .knowledge_graph
            .extract_facts_with_context(
                &ctx.llm,
                &goal.turn,
                ctx.identity.clone(),
                ctx.condensed_memory.as_deref(),
            )
            .await?;

        if !outcome.inserted.is_empty() {
            tracing::info!(
                "Librarian extracted {} facts for topic {}",
                outcome.inserted.len(),
                goal.topic
            );
        }
        if !outcome.pending_confirmation.is_empty() {
            tracing::info!(
                "Librarian has {} facts pending confirmation for topic {}",
                outcome.pending_confirmation.len(),
                goal.topic
            );
        }
        if !outcome.errors.is_empty() {
            tracing::warn!(
                "Librarian extraction errors for topic {}: {:?}",
                goal.topic,
                outcome.errors
            );
        }
        if !outcome.corroborated.is_empty() {
            tracing::debug!(
                "Librarian corroborated {} facts for topic {}",
                outcome.corroborated.len(),
                goal.topic
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_hashable_for_dedupe() {
        let turn = ConversationTurn::new(1, "hi", "hello");
        let g1 = LibrarianGoal::new(7, "user facts", turn.clone());
        let g2 = LibrarianGoal::new(7, "user facts", turn.clone());
        let g3 = LibrarianGoal::new(7, "partner facts", turn.clone());
        assert_eq!(g1, g2);
        assert_ne!(g1, g3);
    }
}
