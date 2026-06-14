//! Agentic retrieval subsystem.
//!
//! The `RetrievalAgent` runs an ephemeral LLM session with only retrieval
//! tools, investigating the knowledge graph and conversation history on
//! behalf of the main agent.

pub mod agent;
pub mod types;

pub use agent::RetrievalAgent;
pub use types::{
    ConversationSnippet, RetrievedContext, RetrievedEntity, RetrievedFact, RetrievedRelation,
};
