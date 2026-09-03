//! LLM condensation pipeline for stable memory facts.
//!
//! Builds a ranked MemorySchema (excluding upcoming and sensitive facts),
//! asks the LLM to condense it into natural prose, validates the output,
//! and caches the result in system_state.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use mimir_core::llm::LlmBackend;
use mimir_core::llm::types::Message;

use crate::KnowledgeError;
use crate::KnowledgeGraph;
use crate::models::memory::{MemoryBucket, MemorySchema};
use crate::queries::memory::BuildMemoryOptions;

const LAST_HASH_KEY: &str = "last_condensed_hash";

/// Orchestrates memory condensation for a single subject entity.
pub struct MemoryCondenser {
    kg: Arc<KnowledgeGraph>,
    llm: Arc<dyn LlmBackend>,
    subject_id: i32,
    char_limit: usize,
    top_n: usize,
}

impl MemoryCondenser {
    pub fn new(
        kg: Arc<KnowledgeGraph>,
        llm: Arc<dyn LlmBackend>,
        subject_id: i32,
        char_limit: usize,
        top_n: usize,
    ) -> Self {
        Self {
            kg,
            llm,
            subject_id,
            char_limit,
            top_n,
        }
    }

    /// Run the condensation pipeline.
    ///
    /// 1. Clears the dirty flag atomically before building the schema.
    /// 2. Builds schema excluding upcoming and sensitive facts.
    /// 3. Computes a hash of the top-N stable facts.
    /// 4. If the hash matches the stored hash, skips the LLM call.
    /// 5. Otherwise, calls the LLM with a pure formatting prompt.
    /// 6. On LLM failure or oversized output, falls back to deterministic rendering.
    /// 7. Stores the result in system_state.
    pub async fn run(&self) -> Result<(), KnowledgeError> {
        // Atomically clear the dirty flag BEFORE we start. Any mutation that
        // arrives during this job will re-set it, and a subsequent run will
        // pick up the new state.
        self.kg.clear_condensation_dirty();

        let opts = BuildMemoryOptions {
            exclude_from_budget: vec![MemoryBucket::Upcoming],
            exclude_sensitive: true,
        };

        let schema = self
            .kg
            .build_memory_schema_with_opts(self.subject_id, self.char_limit, 0.5, opts)
            .await?;

        let top_n_hash = compute_top_n_hash(&schema, self.top_n);

        let stored_hash: Option<String> =
            crate::queries::system_state::get_system_state(self.kg.pool(), LAST_HASH_KEY).await?;

        if stored_hash.as_deref() == Some(&top_n_hash) {
            tracing::info!("memory.condensation: top-N hash unchanged; skipping LLM call");
            return Ok(());
        }

        let deterministic = self
            .kg
            .render_memory_schema(&strip_upcoming(schema.clone()));

        let condensed = match self.call_llm(&deterministic).await {
            Ok(text) if text.chars().count() <= self.char_limit => {
                tracing::info!(
                    "memory.condensation: LLM condensation succeeded ({} chars)",
                    text.chars().count()
                );
                text
            }
            Ok(text) => {
                tracing::warn!(
                    "memory.condensation: LLM output exceeded limit ({} > {} chars); using deterministic fallback",
                    text.chars().count(),
                    self.char_limit
                );
                deterministic
            }
            Err(e) => {
                tracing::warn!(
                    "memory.condensation: LLM call failed ({}); falling back to deterministic renderer",
                    e
                );
                deterministic
            }
        };

        self.kg.set_condensed_memory(&condensed).await?;
        crate::queries::system_state::set_system_state(self.kg.pool(), LAST_HASH_KEY, &top_n_hash)
            .await?;

        tracing::info!(
            "memory.condensation: stored condensed memory ({} chars)",
            condensed.chars().count()
        );
        Ok(())
    }

    async fn call_llm(&self, deterministic_text: &str) -> Result<String, crate::KnowledgeError> {
        let system_msg = Message::system(
            "You format structured personal memory data into concise natural prose. \
             You never invent facts. You never add commentary. You only reorganize and condense the provided data.",
        );

        let user_text = format!(
            "Write a compact factual summary of this person. Group by theme. Max {} characters.\n\n{}",
            self.char_limit, deterministic_text
        );
        let user_msg = Message::user(user_text);

        let (response, _) = self
            .llm
            .chat(vec![system_msg, user_msg], None)
            .await
            .map_err(|e| {
                crate::KnowledgeError::Validation(format!("LLM condensation error: {e}"))
            })?;

        Ok(response)
    }
}

/// Return a schema copy with the upcoming bucket emptied.
/// Used to prevent the LLM from hallucinating about upcoming events.
fn strip_upcoming(mut schema: MemorySchema) -> MemorySchema {
    schema.upcoming.clear();
    schema
}

/// Compute a cheap stable hash of the top-N fact IDs and scores.
fn compute_top_n_hash(schema: &MemorySchema, n: usize) -> String {
    let mut hasher = DefaultHasher::new();
    let facts = schema.all_facts();
    let limit = facts.len().min(n);
    for fact in &facts[..limit] {
        fact.fact_id.hash(&mut hasher);
        // Hash the score bits for stability
        fact.score.to_bits().hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::memory::{MemoryBucket, MemorySchema, RankedFact};

    #[test]
    fn top_n_hash_empty_schema() {
        let schema = MemorySchema::new();
        let hash = compute_top_n_hash(&schema, 20);
        assert!(!hash.is_empty());
    }

    #[test]
    fn top_n_hash_stable_for_same_facts() {
        let fact = RankedFact {
            fact_id: 1,
            subject_name: "A".to_string(),
            relationship_type: "b".to_string(),
            object_display: "C".to_string(),
            valid_from: None,
            valid_until: None,
            confidence: 0.9,
            score: 1.0,
            temporal_boost: 1.0,
            memory_weight: 1.0,
            priority_boost: 1.0,
            centrality_boost: 1.0,
            category_ids: vec![100],
            bucket: MemoryBucket::Identity,
            char_estimate: 10,
        };
        let schema = MemorySchema {
            identity: vec![fact.clone()],
            ..Default::default()
        };
        let h1 = compute_top_n_hash(&schema, 20);
        let h2 = compute_top_n_hash(&schema, 20);
        assert_eq!(h1, h2);
    }

    #[test]
    fn top_n_hash_changes_with_score() {
        let mut fact = RankedFact {
            fact_id: 1,
            subject_name: "A".to_string(),
            relationship_type: "b".to_string(),
            object_display: "C".to_string(),
            valid_from: None,
            valid_until: None,
            confidence: 0.9,
            score: 1.0,
            temporal_boost: 1.0,
            memory_weight: 1.0,
            priority_boost: 1.0,
            centrality_boost: 1.0,
            category_ids: vec![100],
            bucket: MemoryBucket::Identity,
            char_estimate: 10,
        };
        let schema1 = MemorySchema {
            identity: vec![fact.clone()],
            ..Default::default()
        };
        fact.score = 2.0;
        let schema2 = MemorySchema {
            identity: vec![fact],
            ..Default::default()
        };
        let h1 = compute_top_n_hash(&schema1, 20);
        let h2 = compute_top_n_hash(&schema2, 20);
        assert_ne!(h1, h2);
    }

    #[test]
    fn strip_upcoming_clears_bucket() {
        let schema = MemorySchema {
            upcoming: vec![RankedFact {
                fact_id: 1,
                subject_name: "A".to_string(),
                relationship_type: "b".to_string(),
                object_display: "C".to_string(),
                valid_from: None,
                valid_until: None,
                confidence: 0.9,
                score: 1.0,
                temporal_boost: 1.0,
                memory_weight: 1.0,
                priority_boost: 1.0,
                centrality_boost: 1.0,
                category_ids: vec![900],
                bucket: MemoryBucket::Upcoming,
                char_estimate: 10,
            }],
            ..Default::default()
        };
        let stripped = strip_upcoming(schema);
        assert!(stripped.upcoming.is_empty());
    }
}
