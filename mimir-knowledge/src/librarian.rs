//! Librarian Agent — system-driven background fact extraction.
//!
//! The Librarian receives a completed [`ConversationTurn`] together with the
//! core-facts block (the same condensed memory the core agent injects) and
//! extracts/validates/stores structured facts. The user's identity is read
//! from that block by the LLM; the transcript is handed over as labelled
//! `[User]` / `[Assistant]` messages so the agent learns only from what the
//! user said.

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;

use mimir_core::agents::{Agent, AgentContext};
use mimir_core::conversation::{ConversationMessage, ConversationTurn};
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
    pub condensed_memory: Option<String>,
}

impl LibrarianContext {
    /// Create a new LibrarianContext.
    ///
    /// `condensed_memory` is the same core-facts block the core agent injects;
    /// the user's identity is read from it by the LLM, not passed separately.
    pub fn new(
        knowledge_graph: Arc<KnowledgeGraph>,
        llm: Arc<dyn LlmBackend>,
        condensed_memory: Option<String>,
    ) -> Self {
        Self {
            knowledge_graph,
            llm,
            condensed_memory,
        }
    }
}

impl AgentContext for LibrarianContext {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Build the labelled transcript the Librarian analyses from a conversation
/// turn: the last user message followed by the last assistant message.
///
/// Today this is a single user/assistant pair. Returning a `Vec` keeps the
/// door open to sending more context (e.g. the last *N* turns) in future
/// without changing the prompt-builder signature (#139).
fn librarian_messages(turn: &ConversationTurn) -> Vec<ConversationMessage> {
    vec![
        ConversationMessage::user(&turn.user_message),
        ConversationMessage::assistant(&turn.assistant_response),
    ]
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
                &librarian_messages(&goal.turn),
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
