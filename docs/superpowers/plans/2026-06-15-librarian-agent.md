# Librarian Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` or `superpowers:subagent-driven-development` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `spawn_fact_extraction` in `mimir-server/src/routes/chat.rs` with a reusable, goal-driven Librarian Agent that has full conversation-turn, memory, identity, and KB context.

**Architecture:** A generic `Agent` trait + `AgentRuntime` live in `mimir-core`; `LibrarianAgent` lives in `mimir-knowledge` and implements `Agent<Goal = LibrarianGoal>`. The chat route submits a `LibrarianGoal` to the runtime after each non-incognito assistant turn. The runtime dedupes by `(AgentKind, Goal)` and dispatches jobs on a background task. Extraction is still deterministic Rust; the LLM only structures facts via the existing `remember` tool schema, but with a richer prompt.

**Tech Stack:** Rust 2024, `tokio`, `async-trait`, `sqlx`, `serde`, existing `mimir-core`/`mimir-knowledge`/`mimir-server` crates.

---

## Task 1: Shared types for conversation turn and user identity

**Files:**
- Create: `mimir-core/src/conversation.rs`
- Create: `mimir-core/src/identity.rs`
- Modify: `mimir-core/src/lib.rs` to re-export

- [ ] **Step 1: Write the failing test**

Create `mimir-core/src/conversation.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConversationTurn {
    pub user_message: String,
    pub assistant_response: String,
    pub session_id: i64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ConversationTurn {
    pub fn new(session_id: i64, user_message: impl Into<String>, assistant_response: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
            assistant_response: assistant_response.into(),
            session_id,
            timestamp: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_turn_stores_messages() {
        let turn = ConversationTurn::new(42, "hello", "hi there");
        assert_eq!(turn.user_message, "hello");
        assert_eq!(turn.assistant_response, "hi there");
        assert_eq!(turn.session_id, 42);
    }
}
```

Create `mimir-core/src/identity.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserIdentity {
    pub name: &'static str,
    pub entity_id: i32,
}

impl UserIdentity {
    pub fn new(name: &'static str, entity_id: i32) -> Self {
        Self { name, entity_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_holds_name_and_entity_id() {
        let id = UserIdentity::new("Devansh", 7);
        assert_eq!(id.name, "Devansh");
        assert_eq!(id.entity_id, 7);
    }
}
```

Modify `mimir-core/src/lib.rs` to include and re-export them.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mimir-core conversation_turn_stores_messages identity_holds_name_and_entity_id -- --nocapture`
Expected: FAIL (modules/files not yet in lib.rs)

- [ ] **Step 3: Wire modules and run tests again**

Add to `mimir-core/src/lib.rs`:

```rust
pub mod conversation;
pub mod identity;
```

Run: `cargo test -p mimir-core conversation_turn_stores_messages identity_holds_name_and_entity_id -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add mimir-core/src/conversation.rs mimir-core/src/identity.rs mimir-core/src/lib.rs
git commit -m "feat(core): add ConversationTurn and UserIdentity types for agent framework"
```

---

## Task 2: Generic Agent trait and lightweight AgentRuntime

**Files:**
- Create: `mimir-core/src/agents/mod.rs`
- Create: `mimir-core/src/agents/runtime.rs`
- Modify: `mimir-core/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `mimir-core/src/agents/mod.rs`:

```rust
use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use async_trait::async_trait;

/// Marker trait for an agent kind, used for runtime registry and deduplication.
pub trait AgentKind: Send + Sync + Debug + Clone + 'static {
    fn name(&self) -> &'static str;
}

/// Runtime context passed to an agent when it runs.
pub trait AgentContext: Send + Sync + Debug + 'static {
    fn as_any(&self) -> &dyn Any;
}

/// Generic agent contract.
#[async_trait]
pub trait Agent: Send + Sync + 'static {
    /// Concrete goal type that distinguishes one run from another.
    type Goal: Send + Sync + Debug + Clone + Eq + Hash + 'static;

    /// Agent kind identifier used by the runtime.
    fn kind(&self) -> &'static str;

    /// Execute the agent for the given goal.
    async fn run(&self, goal: Self::Goal, ctx: Arc<dyn AgentContext>) -> anyhow::Result<()>;
}
```

Create `mimir-core/src/agents/runtime.rs`:

```rust
use std::any::Any;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::{Agent, AgentContext, AgentKind};

/// Boxed agent stored in the runtime registry.
type BoxedAgent = Arc<dyn Any + Send + Sync>;

/// A pending goal together with its agent kind.
struct PendingKey {
    kind: &'static str,
    goal_hash: u64,
}

impl PartialEq for PendingKey {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.goal_hash == other.goal_hash
    }
}
impl Eq for PendingKey {}
impl Hash for PendingKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        self.goal_hash.hash(state);
    }
}

/// Lightweight in-memory agent runtime.
///
/// Registers agents, dedupes identical `(kind, goal)` submissions, and dispatches
/// them on background tasks when the underlying LLM backend is idle.
#[derive(Clone)]
pub struct AgentRuntime {
    agents: Arc<Mutex<Vec<(String, BoxedAgent)>>>,
    pending: Arc<Mutex<HashSet<PendingKey>>>,
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Register an agent with the runtime.
    pub async fn register<A>(&self, agent: A)
    where
        A: Agent,
    {
        let mut agents = self.agents.lock().await;
        agents.push((agent.kind().to_string(), Arc::new(agent) as BoxedAgent));
    }

    /// Submit a goal to the registered agent of kind `A::kind()`.
    ///
    /// Returns true if the job was newly queued, false if an identical goal is
    /// already pending.
    pub async fn submit<A>(
        &self,
        goal: A::Goal,
        ctx: Arc<dyn AgentContext>,
    ) -> bool
    where
        A: Agent,
    {
        let kind = A::kind_static();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        goal.hash(&mut hasher);
        let key = PendingKey { kind, goal_hash: hasher.finish() };

        let mut pending = self.pending.lock().await;
        if pending.contains(&key) {
            debug!("AgentRuntime: {:?} goal already pending", kind);
            return false;
        }
        pending.insert(key.clone());
        drop(pending);

        let agents = self.agents.lock().await;
        let agent = agents
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, a)| Arc::clone(a))
            .expect("agent kind not registered");
        drop(agents);

        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            let agent = agent.downcast::<A>().expect("agent kind mismatch");
            let result = agent.run(goal, ctx).await;
            pending.lock().await.remove(&key);
            match result {
                Ok(()) => info!("AgentRuntime: {:?} completed successfully", kind),
                Err(e) => warn!("AgentRuntime: {:?} failed: {}", kind, e),
            }
        });

        true
    }
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}
```

Wait — `A::kind_static()` is not defined. Define a helper trait or use a marker. Simpler: make `submit` take `&A` or a `PhantomData`. Better design: `AgentRuntime::submit<A: Agent>(&self, goal: A::Goal, ctx: Arc<dyn AgentContext>)` uses `A::kind()` from trait default, but we need the agent instance. Since we registered an `A`, we can get kind from the stored agent by downcasting. Better:

```rust
pub async fn submit<A: Agent>(
    &self,
    goal: A::Goal,
    ctx: Arc<dyn AgentContext>,
) -> bool {
    let agents = self.agents.lock().await;
    let (_, agent_arc) = agents
        .iter()
        .find(|(k, _)| *k == A::static_kind())
        .expect("agent kind not registered");
    let agent = agent_arc.clone().downcast::<A>().expect("kind mismatch");
    // ...
}
```

Add `fn static_kind() -> &'static str` to `Agent` trait with default body? No, need each impl to provide. Let's add:

```rust
#[async_trait]
pub trait Agent: Send + Sync + 'static {
    type Goal: Send + Sync + Debug + Clone + Eq + Hash + 'static;
    fn kind(&self) -> &'static str;
    async fn run(&self, goal: Self::Goal, ctx: Arc<dyn AgentContext>) -> anyhow::Result<()>;
}

pub trait AgentStaticKind: Agent {
    const KIND: &'static str;
}
```

Then runtime generic requires `A: AgentStaticKind`. Let's adjust tests.

Create `mimir-core/src/agents/runtime.rs` with a simple test:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct TestGoal(String);

    #[derive(Debug, Clone)]
    struct TestAgent {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Agent for TestAgent {
        type Goal = TestGoal;
        fn kind(&self) -> &'static str { "test.agent" }
        async fn run(&self, _goal: TestGoal, _ctx: Arc<dyn AgentContext>) -> anyhow::Result<()> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl AgentStaticKind for TestAgent {
        const KIND: &'static str = "test.agent";
    }

    #[derive(Debug)]
    struct EmptyCtx;
    impl AgentContext for EmptyCtx {
        fn as_any(&self) -> &dyn Any { self }
    }

    #[tokio::test]
    async fn runtime_dispatches_registered_agent() {
        let runtime = AgentRuntime::new();
        let counter = Arc::new(AtomicUsize::new(0));
        runtime.register(TestAgent { counter: Arc::clone(&counter) }).await;
        let queued = runtime.submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx)).await;
        assert!(queued);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn identical_goals_are_deduped() {
        let runtime = AgentRuntime::new();
        let counter = Arc::new(AtomicUsize::new(0));
        runtime.register(TestAgent { counter: Arc::clone(&counter) }).await;
        let queued1 = runtime.submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx)).await;
        let queued2 = runtime.submit::<TestAgent>(TestGoal("a".into()), Arc::new(EmptyCtx)).await;
        assert!(queued1);
        assert!(!queued2);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mimir-core runtime_dispatches_registered_agent identical_goals_are_deduped -- --nocapture`
Expected: FAIL (agents module not in lib.rs)

- [ ] **Step 3: Wire modules and run tests again**

Add to `mimir-core/src/lib.rs`:

```rust
pub mod agents;
```

Add to `mimir-core/Cargo.toml` if `anyhow` is not already a dependency. Check first.

Run: `cargo test -p mimir-core runtime_dispatches_registered_agent identical_goals_are_deduped -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add mimir-core/src/agents/mod.rs mimir-core/src/agents/runtime.rs mimir-core/src/lib.rs
git commit -m "feat(core): add generic Agent trait and AgentRuntime"
```

---

## Task 3: LibrarianAgent with rich extraction context

**Files:**
- Create: `mimir-knowledge/src/librarian.rs`
- Modify: `mimir-knowledge/src/extract.rs` (add `extract_facts_with_context`)
- Modify: `mimir-knowledge/src/lib.rs` (register public API)

- [ ] **Step 1: Write the failing test**

Create `mimir-knowledge/src/librarian.rs` initial module with types and a test:

```rust
use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mimir_core::agents::{Agent, AgentContext};
use mimir_core::conversation::ConversationTurn;
use mimir_core::identity::UserIdentity;
use mimir_core::llm::backend::LlmBackend;

use crate::extract::ExtractionOutcome;
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

/// Context passed to the LibrarianAgent by the runtime.
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
    fn as_any(&self) -> &dyn Any { self }
}

/// Background agent that extracts structured facts from a completed conversation turn.
pub struct LibrarianAgent;

impl LibrarianAgent {
    pub fn new() -> Self { Self }
}

impl Default for LibrarianAgent {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Agent for LibrarianAgent {
    type Goal = LibrarianGoal;

    fn kind(&self) -> &'static str { "librarian" }

    async fn run(&self, goal: LibrarianGoal, ctx: Arc<dyn AgentContext>) -> anyhow::Result<()> {
        let ctx = ctx
            .as_any()
            .downcast_ref::<LibrarianContext>()
            .ok_or_else(|| anyhow::anyhow!("LibrarianAgent requires LibrarianContext"))?;
        let outcome = ctx
            .knowledge_graph
            .extract_facts_with_context(&ctx.llm, &goal.turn, ctx.identity, ctx.condensed_memory.as_deref())
            .await?;
        if !outcome.inserted.is_empty() {
            tracing::info!("Librarian extracted {} facts for topic {}", outcome.inserted.len(), goal.topic);
        }
        if !outcome.pending_confirmation.is_empty() {
            tracing::info!(
                "Librarian has {} facts pending confirmation for topic {}",
                outcome.pending_confirmation.len(),
                goal.topic
            );
        }
        if !outcome.errors.is_empty() {
            tracing::warn!("Librarian extraction errors for topic {}: {:?}", goal.topic, outcome.errors);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;

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
```

Modify `mimir-knowledge/src/extract.rs` to add `extract_facts_with_context`:

```rust
pub async fn extract_facts_with_context(
    kg: &KnowledgeGraph,
    llm: &Arc<dyn LlmBackend>,
    turn: &mimir_core::conversation::ConversationTurn,
    identity: mimir_core::identity::UserIdentity,
    condensed_memory: Option<&str>,
) -> Result<ExtractionOutcome, KnowledgeError> {
    let now = kg.now();
    let prompt = build_contextual_extraction_prompt(kg, turn, identity, condensed_memory).await?;
    let transcript = format!(
        "User: {}\nAssistant: {}",
        turn.user_message, turn.assistant_response
    );
    let messages = vec![
        mimir_core::llm::types::Message::system(prompt),
        mimir_core::llm::types::Message::user(transcript),
    ];
    let tool = remember_tool_schema();

    let (assistant_msg, _usage) = llm
        .chat_message(messages, Some(vec![tool]))
        .await
        .map_err(|e| KnowledgeError::Validation(format!("LLM call failed: {}", e)))?;

    // parse same as extract_facts
    let extracted: RememberOutput = parse_remember_output(assistant_msg)?;
    process_extracted_facts(kg, extracted, now).await
}
```

Refactor `extract_facts` to call the same parser/processor with the old prompt.

Add to `mimir-knowledge/src/lib.rs`:

```rust
pub mod librarian;
```

And add method on `KnowledgeGraph`:

```rust
pub async fn extract_facts_with_context(
    &self,
    llm: &Arc<dyn mimir_core::llm::backend::LlmBackend>,
    turn: &mimir_core::conversation::ConversationTurn,
    identity: mimir_core::identity::UserIdentity,
    condensed_memory: Option<&str>,
) -> Result<crate::extract::ExtractionOutcome, KnowledgeError> {
    extract::extract_facts_with_context(self, llm, turn, identity, condensed_memory).await
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mimir-knowledge goal_hashable_for_dedupe -- --nocapture`
Expected: FAIL (librarian module not in lib.rs, extract_facts_with_context missing)

- [ ] **Step 3: Implement and run tests**

Implement the above, ensuring `extract.rs` parser/processor is refactored cleanly.

Run: `cargo test -p mimir-knowledge goal_hashable_for_dedupe -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add mimir-knowledge/src/librarian.rs mimir-knowledge/src/extract.rs mimir-knowledge/src/lib.rs
git commit -m "feat(knowledge): add LibrarianAgent and contextual fact extraction"
```

---

## Task 4: Wire LibrarianAgent into server state and chat route

**Files:**
- Modify: `mimir-server/src/state.rs`
- Modify: `mimir-server/src/routes/chat.rs`

- [ ] **Step 1: Write the failing test**

Create or extend a server integration test in `mimir-server/src/lib.rs` that:
1. Builds `AppState` with a mock LLM.
2. Sends a non-incognito chat request.
3. Sleeps briefly.
4. Asserts that the agent runtime has no pending jobs and that facts were extracted (by querying KG directly).

The test will fail until the wiring is in place.

- [ ] **Step 2: Wire runtime and agent into AppState**

In `mimir-server/src/state.rs`:
1. Add `pub agent_runtime: Arc<mimir_core::agents::AgentRuntime>` to `AppState`.
2. After creating `knowledge_graph`, construct `AgentRuntime::new()` and register `LibrarianAgent`.

```rust
let agent_runtime = Arc::new(mimir_core::agents::AgentRuntime::new());
agent_runtime
    .register::<mimir_knowledge::librarian::LibrarianAgent>(mimir_knowledge::librarian::LibrarianAgent::new())
    .await;
```

3. Add `agent_runtime` to the returned `AppState`.

- [ ] **Step 3: Replace spawn_fact_extraction with goal submission**

In `mimir-server/src/routes/chat.rs`:
1. Delete `spawn_fact_extraction`.
2. After `add_assistant_message`, build a `ConversationTurn` and a `LibrarianGoal`:

```rust
if !incognito && !full_response.is_empty() {
    if let Err(e) = state_clone.context_manager.add_assistant_message(...).await {
        error!("failed to persist assistant message: {e}");
    }

    let turn = ConversationTurn::new(session_id_clone, user_message.clone(), full_response.clone());
    let goal = mimir_knowledge::librarian::LibrarianGoal::new(
        state_clone.user_entity_id.unwrap_or_else(|| -1),
        "chat-turn-extraction",
        turn,
    );
    let ctx = mimir_knowledge::librarian::LibrarianContext::new(
        Arc::clone(&state_clone.knowledge_graph),
        Arc::clone(&llm_clone),
        mimir_core::identity::UserIdentity::new("user", state_clone.user_entity_id.unwrap_or(-1)),
        state_clone.knowledge_graph.get_condensed_memory().await.ok().flatten(),
    );
    let runtime = Arc::clone(&state_clone.agent_runtime);
    let _ = runtime.submit::<mimir_knowledge::librarian::LibrarianAgent>(goal, Arc::new(ctx)).await;
}
```

- [ ] **Step 4: Run server tests**

Run: `cargo test -p mimir-server chat_librarian -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add mimir-server/src/state.rs mimir-server/src/routes/chat.rs
git commit -m "feat(server): wire LibrarianAgent into chat route"
```

---

## Task 5: Add integration tests for LibrarianAgent

**Files:**
- Create: `mimir-knowledge/tests/librarian_agent.rs`

- [ ] **Step 1: Write the failing test**

Create `mimir-knowledge/tests/librarian_agent.rs`:

```rust
use std::sync::Arc;

use mimir_core::agents::{Agent, AgentContext};
use mimir_core::conversation::ConversationTurn;
use mimir_core::identity::UserIdentity;
use mimir_core::llm::{MockLlmClient, types::Message};
use mimir_knowledge::librarian::{LibrarianAgent, LibrarianContext, LibrarianGoal};
use mimir_knowledge::models::entity::EntityType;

#[sqlx::test]
async fn librarian_extracts_fact_from_conversation_turn(pool: sqlx::SqlitePool) {
    // Note: sqlx::test macro usage needs tempdir for FTS5; use KnowledgeGraph::init_with_clock if needed.
}
```

The test will be fleshed out once helper patterns from existing `mimir-knowledge` tests are reviewed. For now, create a minimal placeholder that fails to compile to drive implementation.

- [ ] **Step 2: Implement full test**

Using a temp directory and `KnowledgeGraph::init_with_clock`, create a user entity, build a `ConversationTurn`, mock the LLM to return a `RememberOutput`, and assert the fact is inserted.

- [ ] **Step 3: Run tests**

Run: `cargo test -p mimir-knowledge --test librarian_agent -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add mimir-knowledge/tests/librarian_agent.rs
git commit -m "test(knowledge): add LibrarianAgent integration tests"
```

---

## Task 6: Update documentation

**Files:**
- Create: `docs/librarian-agent.md`
- Modify: `docs/wiki/what-works-now.md` (or create)
- Modify: `README.md`
- Modify: `VISION/02-Knowledge-Graph/Learning-Modes.md` or `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md`

- [ ] **Step 1: Create technical doc**

`docs/librarian-agent.md` covers:
- What the Librarian Agent does
- `Agent` trait and `AgentRuntime` overview
- `LibrarianGoal` shape
- Data flow from chat route to KG
- How to test and extend

- [ ] **Step 2: Update wiki doc**

Add to `docs/wiki/what-works-now.md`:
- "After a chat turn, the Librarian Agent extracts facts in the background using full transcript, memory, identity, and KB context."

- [ ] **Step 3: Update README and VISION**

Update README feature list. Add goal-directed research as future stretch goal in VISION Phase 2 roadmap.

- [ ] **Step 4: Commit**

```bash
git add docs/librarian-agent.md docs/wiki/what-works-now.md README.md VISION/...
git commit -m "docs: add Librarian Agent architecture and update wiki/roadmap"
```

---

## Task 7: Code review, version bump, and draft PR

- [ ] **Step 1: Run code review pass**

Review all changed files for:
- Code quality
- Performance
- Security
- Doc comments
- DRY compliance
- Type consistency
- VISION/AGENTS compliance

Action every finding.

- [ ] **Step 2: Run verification**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- [ ] **Step 3: Bump workspace version**

Update version in all `Cargo.toml` files (minor bump) and update `CHANGELOG.md`.

- [ ] **Step 4: Commit and push**

```bash
git add -A
git commit -m "chore(release): bump version to X.Y.Z"
git push origin feat/librarian-agent-issue-130
```

- [ ] **Step 5: Open draft PR**

Use `gh` or GitHub app to create a draft PR:

```bash
gh pr create --draft --title "feat: Librarian Agent — system-driven background fact extraction (Issue #130)" --body "Closes #130

## Summary
Replaces fire-and-forget `spawn_fact_extraction` with a reusable `LibrarianAgent` running on a generic `AgentRuntime`. The agent receives the full conversation turn, condensed memory, user identity, and KB snapshot to extract facts in the background.

## Documentation
- docs/librarian-agent.md
- docs/wiki/what-works-now.md
- README.md
- VISION/09-Roadmap/Phase-2-Knowledge-Graph.md

## Testing
- Unit tests for AgentRuntime dedupe and dispatch
- mimir-knowledge integration tests for LibrarianAgent
- mimir-server chat integration test"
```

---

## Notes for Implementer

- Keep all control logic in Rust; the LLM only structures facts via the existing `remember` tool schema.
- Do not change the public API surface of `KnowledgeGraph::extract_facts` (keep it as a wrapper for compatibility).
- All workspace crates must remain on `#![deny(unsafe_code)]`.
- Prefer `Arc<dyn LlmBackend>` patterns already used in the codebase.
- Use the existing `MockLlmClient` for tests.
