//! Hook handlers registered by the daemon (issue #386).
//!
//! Each handler is a deterministic Rust pipeline: the hooks engine owns
//! queueing, debounce, gating, and retry; the handler only receives the
//! accumulated payload and returns an outcome. No handler logic lives in
//! prompts.

#![deny(unsafe_code)]

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use mimir_core::conversation::{ConversationMessage, ConversationTurn};
use mimir_core::hooks::{HookContext, HookHandler, HookOutcome};
use mimir_core::llm::LlmBackend;

/// Merge a newly completed chat turn into the accumulated pending payload
/// for the `remember.chat` hook (true debounce: consecutive turns within the
/// window become one extraction over the accumulated transcript).
pub fn merge_chat_turns(
    old: Arc<dyn Any + Send + Sync>,
    new: Arc<dyn Any + Send + Sync>,
) -> Arc<dyn Any + Send + Sync> {
    let Ok(mut turns) = old.downcast::<Vec<ConversationTurn>>() else {
        return new;
    };
    let new_turns = match new.downcast::<Vec<ConversationTurn>>() {
        Ok(turns) => turns,
        Err(new) => return new,
    };
    Arc::make_mut(&mut turns).extend(new_turns.iter().cloned());
    turns
}

/// Handler for the `memory.condensation` hook: rebuilds the condensed
/// memory block from the knowledge graph when facts change.
pub struct CondensationHandler {
    kg: Arc<mimir_knowledge::KnowledgeGraph>,
    llm: Arc<dyn LlmBackend>,
    user_entity_id: Option<i32>,
    char_limit: usize,
    top_n: usize,
}

impl CondensationHandler {
    pub fn new(
        kg: Arc<mimir_knowledge::KnowledgeGraph>,
        llm: Arc<dyn LlmBackend>,
        user_entity_id: Option<i32>,
        char_limit: usize,
        top_n: usize,
    ) -> Self {
        Self {
            kg,
            llm,
            user_entity_id,
            char_limit,
            top_n,
        }
    }
}

#[async_trait]
impl HookHandler for CondensationHandler {
    async fn run(&self, _payload: Arc<dyn Any + Send + Sync>, _ctx: HookContext) -> HookOutcome {
        let Some(subject_id) = self.user_entity_id else {
            tracing::debug!("memory.condensation: no user entity configured; skipping");
            return HookOutcome::Success;
        };
        let condenser = mimir_knowledge::condensation::MemoryCondenser::new(
            Arc::clone(&self.kg),
            Arc::clone(&self.llm),
            subject_id,
            self.char_limit,
            self.top_n,
        );
        match condenser.run().await {
            Ok(()) => HookOutcome::Success,
            Err(error) => {
                warn!("memory.condensation hook failed: {error}");
                HookOutcome::TerminalFailure
            }
        }
    }
}

/// Handler for the `remember.chat` hook: runs the Librarian extraction
/// pipeline over the accumulated conversation turns.
pub struct ChatLearningHandler {
    kg: Arc<mimir_knowledge::KnowledgeGraph>,
    llm: Arc<dyn LlmBackend>,
}

impl ChatLearningHandler {
    pub fn new(kg: Arc<mimir_knowledge::KnowledgeGraph>, llm: Arc<dyn LlmBackend>) -> Self {
        Self { kg, llm }
    }
}

#[async_trait]
impl HookHandler for ChatLearningHandler {
    async fn run(&self, payload: Arc<dyn Any + Send + Sync>, _ctx: HookContext) -> HookOutcome {
        let Ok(turns) = payload.downcast::<Vec<ConversationTurn>>() else {
            warn!("remember.chat hook: unexpected payload type; dropping instance");
            return HookOutcome::TerminalFailure;
        };
        let messages: Vec<ConversationMessage> = turns
            .iter()
            .flat_map(|turn| {
                [
                    ConversationMessage::user(turn.user_message.clone()),
                    ConversationMessage::assistant(turn.assistant_response.clone()),
                ]
            })
            .collect();
        let condensed = match self.kg.get_condensed_memory().await {
            Ok(memory) => memory,
            Err(error) => {
                warn!("remember.chat hook: failed to read condensed memory: {error}");
                None
            }
        };
        match mimir_knowledge::extract::extract_facts_with_context(
            &self.kg,
            &self.llm,
            &messages,
            condensed.as_deref(),
        )
        .await
        {
            Ok(outcome) => {
                tracing::debug!(
                    "remember.chat hook: inserted {} fact(s), {} pending confirmation",
                    outcome.inserted.len(),
                    outcome.pending_confirmation.len()
                );
                HookOutcome::Success
            }
            Err(error) => {
                warn!("remember.chat hook failed: {error}");
                HookOutcome::TerminalFailure
            }
        }
    }
}
