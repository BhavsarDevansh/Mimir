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
        Err(_) => {
            // A malformed trigger payload must not discard the accumulated
            // transcript: keep the existing turns so only the bad payload is
            // lost (one unexpected payload type would otherwise drop a whole
            // debounced burst as a terminal failure).
            warn!("remember.chat merge: unexpected new payload type; keeping accumulated turns");
            return turns;
        }
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
                // Transient LLM/provider failures must not lose the
                // accumulated transcript: the hook's retry policy (issue
                // #386) re-enqueues the instance with backoff so the burst
                // is re-extracted instead of dropped.
                HookOutcome::RetryableFailure
            }
        }
    }
}

/// Handler for the `session.compaction` hook (issue #279): summarises the
/// oldest complete turns beyond the configured window, stores the summary on
/// the session, and deletes the summarised messages.
pub struct SessionCompactionHandler {
    context: Arc<mimir_core::context::ContextManager>,
    llm: Arc<dyn LlmBackend>,
    max_turns: u16,
}

impl SessionCompactionHandler {
    pub fn new(
        context: Arc<mimir_core::context::ContextManager>,
        llm: Arc<dyn LlmBackend>,
        max_turns: u16,
    ) -> Self {
        Self {
            context,
            llm,
            max_turns,
        }
    }
}

#[async_trait]
impl HookHandler for SessionCompactionHandler {
    async fn run(&self, payload: Arc<dyn Any + Send + Sync>, _ctx: HookContext) -> HookOutcome {
        let Ok(turns) = payload.downcast::<Vec<ConversationTurn>>() else {
            warn!("session.compaction hook: unexpected payload type; dropping instance");
            return HookOutcome::TerminalFailure;
        };
        let Some(session_id) = turns.first().map(|turn| turn.session_id) else {
            warn!("session.compaction hook: empty turn payload; dropping instance");
            return HookOutcome::TerminalFailure;
        };
        let compactor = mimir_core::context::SessionCompactor::new(
            Arc::clone(&self.context),
            Arc::clone(&self.llm),
            self.max_turns,
        );
        match compactor.compact_session(session_id).await {
            Ok(_) => HookOutcome::Success,
            Err(mimir_core::context::ContextError::SessionNotFound(_)) => {
                // The session was deleted between the trigger and the run;
                // retrying cannot succeed.
                HookOutcome::TerminalFailure
            }
            Err(error) => {
                warn!("session.compaction hook failed: {error}");
                // Transient DB failures are re-enqueued with backoff so a
                // burst of turns is not left unsummarised.
                HookOutcome::RetryableFailure
            }
        }
    }
}
