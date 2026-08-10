use crate::graph::KnowledgeGraph;
use crate::*;

use crate::models::fact::FactStatus;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Centrality cache
    // ------------------------------------------------------------------

    /// Clear the centrality cache, forcing a rebuild on next use.
    pub async fn set_centrality_dirty(&self) {
        let mut cache = self.centrality_cache.write().await;
        cache.clear();
    }

    /// Populate the centrality cache by scanning all facts in the graph.
    /// Called once on first memory build; subsequent builds use cached values.
    pub async fn populate_centrality_cache(&self) -> Result<(), KnowledgeError> {
        let mut cache = self.centrality_cache.write().await;
        let rows: Vec<(i32, i64)> = sqlx::query_as(
            r#"SELECT entity_id, COUNT(*) FROM (
                SELECT subject_id AS entity_id FROM facts WHERE fact_status_id NOT IN (?, ?)
                UNION ALL
                SELECT object_id AS entity_id FROM facts WHERE object_id IS NOT NULL AND fact_status_id NOT IN (?, ?)
            )
            GROUP BY entity_id"#,
        )
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .fetch_all(&self.pool)
        .await?;

        for (entity_id, count) in rows {
            let boost = 1.0 + (count as f32).min(50.0) / 50.0;
            cache.insert(entity_id, boost);
        }

        Ok(())
    }

    /// Increment centrality for an entity (used on fact insertion).
    pub async fn bump_centrality(&self, entity_id: i32) {
        let mut lock = self.centrality_cache.write().await;
        let entry = lock.entry(entity_id).or_insert(1.0);
        let count = ((*entry - 1.0) * 50.0 + 1.0).min(50.0);
        *entry = 1.0 + count / 50.0;
    }

    /// Decrement centrality for an entity (used on fact forget).
    pub async fn drop_centrality(&self, entity_id: i32) {
        let mut lock = self.centrality_cache.write().await;
        if let Some(entry) = lock.get_mut(&entity_id) {
            let count = ((*entry - 1.0) * 50.0 - 1.0).max(0.0);
            *entry = 1.0 + count / 50.0;
            if *entry <= 1.0 {
                lock.remove(&entity_id);
            }
        }
    }
}
