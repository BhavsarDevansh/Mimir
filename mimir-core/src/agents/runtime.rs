//! Lightweight in-memory agent runtime.
//!
//! Registers agents, dedupes identical `(kind, goal)` submissions, and
//! dispatches them on background tasks. This is the first iteration of the
//! agent executor; durable scheduling through the [`JobQueue`](crate::job_queue::JobQueue)
//! is intentionally deferred to a later issue.

use std::any::Any;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tokio::sync::Mutex;
#[cfg(test)]
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::{Agent, AgentContext};

/// Boxed agent stored in the runtime registry.
type BoxedAgent = Arc<dyn Any + Send + Sync>;

/// A pending goal together with its agent kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PendingKey {
    kind: String,
    goal_hash: u64,
}

impl PendingKey {
    fn new(kind: &'static str, goal_hash: u64) -> Self {
        Self {
            kind: kind.to_string(),
            goal_hash,
        }
    }
}

/// Lightweight in-memory agent runtime.
///
/// Registers agents, dedupes by `(agent kind, goal)`, and dispatches them on
/// background tasks.
#[derive(Clone, Debug)]
pub struct AgentRuntime {
    agents: Arc<Mutex<Vec<(String, BoxedAgent)>>>,
    pending: Arc<Mutex<HashSet<PendingKey>>>,
    /// Test-only sequence of dispatched agent-task exits.
    #[cfg(test)]
    task_exits: Arc<watch::Sender<u64>>,
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRuntime {
    /// Subscribe to the test-only dispatched agent-task exit sequence.
    #[cfg(test)]
    pub fn task_exit_rx(&self) -> watch::Receiver<u64> {
        self.task_exits.subscribe()
    }

    /// Create a new runtime.
    pub fn new() -> Self {
        Self {
            agents: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(Mutex::new(HashSet::new())),
            #[cfg(test)]
            task_exits: Arc::new(watch::channel(0).0),
        }
    }

    /// Register an agent with the runtime.
    ///
    /// Only one agent per [`Agent::KIND`] is supported. Registering a second
    /// agent with the same kind overwrites the previous one.
    pub async fn register<A>(&self, agent: A)
    where
        A: Agent,
    {
        let mut agents = self.agents.lock().await;
        let kind = A::KIND.to_string();
        if let Some(pos) = agents.iter().position(|(k, _)| k == &kind) {
            agents[pos] = (kind, Arc::new(agent));
        } else {
            agents.push((kind, Arc::new(agent)));
        }
    }

    /// Submit a goal to the registered agent of kind `A`.
    ///
    /// Returns `true` if the job was newly queued, `false` if an identical
    /// `(kind, goal)` is already pending.
    ///
    /// # Panics
    ///
    /// Panics if the agent kind has not been registered. This is a programmer
    /// error because callers should always register an agent before submitting.
    pub async fn submit<A>(&self, goal: A::Goal, ctx: Arc<dyn AgentContext>) -> bool
    where
        A: Agent,
    {
        let kind = A::KIND;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        goal.hash(&mut hasher);
        let key = PendingKey::new(kind, hasher.finish());

        {
            let mut pending = self.pending.lock().await;
            if pending.contains(&key) {
                debug!("AgentRuntime: {} goal already pending", kind);
                return false;
            }
            pending.insert(key.clone());
        }

        let agent = {
            let agents = self.agents.lock().await;
            agents
                .iter()
                .find(|(k, _)| k == kind)
                .map(|(_, a)| Arc::clone(a))
                .expect("agent kind not registered")
        };

        let pending = Arc::clone(&self.pending);
        #[cfg(test)]
        let task_exits = Arc::clone(&self.task_exits);
        tokio::spawn(async move {
            let task = tokio::spawn(async move {
                let agent = agent
                    .downcast_ref::<A>()
                    .expect("agent kind mismatch in runtime");
                agent.run(goal, ctx).await
            });
            let outcome = task.await;
            pending.lock().await.remove(&key);
            match outcome {
                Ok(Ok(())) => info!("AgentRuntime: {} completed successfully", kind),
                Ok(Err(e)) => warn!("AgentRuntime: {} failed: {}", kind, e),
                Err(_) => warn!("AgentRuntime: {} panicked", kind),
            }
            #[cfg(test)]
            crate::test_sync::increment_watch(&task_exits);
        });

        true
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::test_sync::wait_for_watch_minimum;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct TestGoal(String);

    #[derive(Debug, Clone)]
    struct TestAgent {
        counter: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct EmptyCtx;

    impl AgentContext for EmptyCtx {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[async_trait]
    impl Agent for TestAgent {
        type Goal = TestGoal;
        const KIND: &'static str = "test.agent";

        async fn run(&self, _goal: TestGoal, _ctx: Arc<dyn AgentContext>) -> anyhow::Result<()> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Debug, Clone)]
    struct PanicAgent;

    #[async_trait]
    impl Agent for PanicAgent {
        type Goal = TestGoal;
        const KIND: &'static str = "panic.agent";

        async fn run(&self, _goal: TestGoal, _ctx: Arc<dyn AgentContext>) -> anyhow::Result<()> {
            panic!("simulated agent panic");
        }
    }

    #[tokio::test]
    async fn runtime_dispatches_registered_agent() {
        let runtime = AgentRuntime::new();
        let counter = Arc::new(AtomicUsize::new(0));
        runtime
            .register(TestAgent {
                counter: Arc::clone(&counter),
            })
            .await;
        let queued = runtime
            .submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx))
            .await;
        assert!(queued);
        let mut task_exits = runtime.task_exit_rx();
        wait_for_watch_minimum(&mut task_exits, 1).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn identical_goals_are_deduped() {
        let runtime = AgentRuntime::new();
        let counter = Arc::new(AtomicUsize::new(0));
        runtime
            .register(TestAgent {
                counter: Arc::clone(&counter),
            })
            .await;
        let queued1 = runtime
            .submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx))
            .await;
        let queued2 = runtime
            .submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx))
            .await;
        assert!(queued1);
        assert!(!queued2);
        let mut task_exits = runtime.task_exit_rx();
        wait_for_watch_minimum(&mut task_exits, 1).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_goals_are_not_deduped() {
        let runtime = AgentRuntime::new();
        let counter = Arc::new(AtomicUsize::new(0));
        runtime
            .register(TestAgent {
                counter: Arc::clone(&counter),
            })
            .await;
        runtime
            .submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx))
            .await;
        runtime
            .submit::<TestAgent>(TestGoal("b".into()), Arc::new(EmptyCtx))
            .await;
        let mut task_exits = runtime.task_exit_rx();
        wait_for_watch_minimum(&mut task_exits, 2).await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn panicked_goal_is_removed_from_pending() {
        let runtime = AgentRuntime::new();
        runtime.register(PanicAgent).await;
        let queued1 = runtime
            .submit::<PanicAgent>(TestGoal("a".into()), Arc::new(EmptyCtx))
            .await;
        assert!(queued1);
        let mut task_exits = runtime.task_exit_rx();
        wait_for_watch_minimum(&mut task_exits, 1).await;
        let queued2 = runtime
            .submit::<PanicAgent>(TestGoal("a".into()), Arc::new(EmptyCtx))
            .await;
        assert!(queued2);
    }
}
