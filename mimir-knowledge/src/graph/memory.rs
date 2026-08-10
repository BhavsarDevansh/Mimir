use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Memory API delegates
    // ------------------------------------------------------------------

    /// Build a ranked memory schema for the given subject.
    pub async fn build_memory_schema(
        &self,
        subject_id: i32,
        budget: usize,
        min_confidence: f32,
    ) -> Result<models::memory::MemorySchema, KnowledgeError> {
        self.build_memory_schema_with_opts(
            subject_id,
            budget,
            min_confidence,
            queries::memory::BuildMemoryOptions::default(),
        )
        .await
    }

    /// Build a ranked memory schema with filtering options.
    pub async fn build_memory_schema_with_opts(
        &self,
        subject_id: i32,
        budget: usize,
        min_confidence: f32,
        opts: queries::memory::BuildMemoryOptions,
    ) -> Result<models::memory::MemorySchema, KnowledgeError> {
        {
            let cache = self.centrality_cache.read().await;
            if !cache.is_empty() {
                let schema = queries::memory::build_memory_schema_with_opts(
                    &self.pool,
                    subject_id,
                    budget,
                    min_confidence,
                    self.now(),
                    &cache,
                    opts,
                )
                .await?;
                return Ok(schema);
            }
        }
        self.populate_centrality_cache().await?;
        let cache = self.centrality_cache.read().await;
        let schema = queries::memory::build_memory_schema_with_opts(
            &self.pool,
            subject_id,
            budget,
            min_confidence,
            self.now(),
            &cache,
            opts,
        )
        .await?;
        Ok(schema)
    }

    /// Render a MemorySchema into deterministic plain text.
    pub fn render_memory_schema(&self, schema: &models::memory::MemorySchema) -> String {
        queries::memory::render_memory_schema(schema)
    }

    /// Render the upcoming events section for a subject entity.
    pub async fn render_upcoming_section(
        &self,
        subject_id: i32,
        days_ahead: i64,
        limit: usize,
    ) -> Result<String, KnowledgeError> {
        queries::memory::render_upcoming_section(
            &self.pool,
            subject_id,
            self.now(),
            days_ahead,
            limit,
        )
        .await
    }

    /// Read the cached condensed memory from system_state.
    pub async fn get_condensed_memory(&self) -> Result<Option<String>, KnowledgeError> {
        queries::system_state::get_system_state(&self.pool, "condensed_memory").await
    }

    /// Write condensed memory to system_state.
    pub async fn set_condensed_memory(&self, text: &str) -> Result<(), KnowledgeError> {
        queries::system_state::set_system_state(&self.pool, "condensed_memory", text).await
    }
}
