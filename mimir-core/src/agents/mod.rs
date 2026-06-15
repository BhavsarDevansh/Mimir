//! Generic agent framework for Mimir.
//!
//! Agents are autonomous background workers registered with an
//! [`AgentRuntime`]. Each agent has a typed [`Goal`](Agent::Goal) and a
//! static kind. Identical `(kind, goal)` submissions are deduplicated while
//! the first run is still pending.

use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use async_trait::async_trait;

mod runtime;
pub use runtime::AgentRuntime;

/// Runtime context passed to an agent when it runs.
///
/// Concrete contexts implement this trait and are downcast by the agent.
pub trait AgentContext: Send + Sync + Debug + 'static {
    fn as_any(&self) -> &dyn Any;
}

/// Generic agent contract.
#[async_trait]
pub trait Agent: Send + Sync + 'static {
    /// Concrete goal type that distinguishes one run from another.
    ///
    /// Must implement [`Hash`] so the runtime can dedupe identical goals.
    type Goal: Send + Sync + Debug + Clone + Eq + Hash + 'static;

    /// Stable agent kind identifier used by the runtime registry.
    const KIND: &'static str;

    /// Human-readable kind identifier.
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    /// Execute the agent for the given goal.
    async fn run(&self, goal: Self::Goal, ctx: Arc<dyn AgentContext>) -> anyhow::Result<()>;
}
