//! Lightweight Rust-native inference engine.

pub mod rules;

use async_trait::async_trait;

use crate::KnowledgeGraph;
use crate::models::fact::{Fact, NewFact};

/// Context shared across a single inference cascade to detect cycles.
pub struct CascadeContext {
    seen: std::collections::HashSet<(i32, i16, Option<i32>, Option<String>)>,
}

impl Default for CascadeContext {
    fn default() -> Self {
        Self::new()
    }
}

impl CascadeContext {
    pub fn new() -> Self {
        Self {
            seen: std::collections::HashSet::new(),
        }
    }

    pub fn insert(
        &mut self,
        subject_id: i32,
        predicate_id: i16,
        object_id: Option<i32>,
        object_literal: Option<String>,
    ) {
        self.seen
            .insert((subject_id, predicate_id, object_id, object_literal));
    }

    pub fn contains(
        &self,
        subject_id: i32,
        predicate_id: i16,
        object_id: Option<i32>,
        object_literal: Option<&str>,
    ) -> bool {
        self.seen.contains(&(
            subject_id,
            predicate_id,
            object_id,
            object_literal.map(|s| s.to_string()),
        ))
    }
}

/// A single inference rule.
#[async_trait]
pub trait InferenceRule: Send + Sync {
    /// Evaluate this rule against the given fact.
    /// Returns zero or more inferred `NewFact`s.
    async fn evaluate(
        &self,
        fact: &Fact,
        kg: &KnowledgeGraph,
    ) -> Result<Vec<NewFact>, crate::KnowledgeError>;
}

/// Engine that holds and runs all registered inference rules.
pub struct RuleEngine {
    rules: Vec<Box<dyn InferenceRule>>,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn register(&mut self, rule: Box<dyn InferenceRule>) {
        self.rules.push(rule);
    }

    /// Evaluate all rules against a single newly-inserted fact.
    pub async fn evaluate_insert(
        &self,
        fact: &Fact,
        kg: &KnowledgeGraph,
        _ctx: &mut CascadeContext,
    ) -> Result<Vec<NewFact>, crate::KnowledgeError> {
        let mut results = Vec::new();
        for rule in &self.rules {
            let mut inferred = rule.evaluate(fact, kg).await?;
            results.append(&mut inferred);
        }
        Ok(results)
    }

    /// Batch re-evaluation: iterate all Active/Inferred facts and run rules.
    pub async fn evaluate_batch(
        &self,
        kg: &KnowledgeGraph,
    ) -> Result<Vec<NewFact>, crate::KnowledgeError> {
        let mut results = Vec::new();
        // Fetch all Active and Inferred facts.
        let facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
            "SELECT id, subject_id, predicate_id, object_id, object_literal, \
             valid_from, valid_until, confidence, fact_status_id, inferred, \
             inference_depth, stale_confidence, created_at, updated_at \
             FROM facts \
             WHERE fact_status_id IN (?, ?)",
        )
        .bind(crate::models::fact::FactStatus::Active as i16)
        .bind(crate::models::fact::FactStatus::Inferred as i16)
        .fetch_all(kg.pool())
        .await?;

        for fact in &facts {
            for rule in &self.rules {
                let mut inferred = rule.evaluate(fact, kg).await?;
                results.append(&mut inferred);
            }
        }
        Ok(results)
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}
