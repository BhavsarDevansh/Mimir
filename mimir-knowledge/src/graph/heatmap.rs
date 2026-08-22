//! Heatmap aggregation facade delegate (issue #69).

use crate::KnowledgeError;
use crate::queries::heatmap::HeatmapData;

impl super::KnowledgeGraph {
    /// Snapshot the knowledge graph's density: totals, entity/predicate/
    /// temporal/confidence distributions. Forgotten (trashed) facts are
    /// excluded. Backs `mimir kb heatmap`.
    pub async fn heatmap(&self) -> Result<HeatmapData, KnowledgeError> {
        crate::queries::heatmap::heatmap(&self.pool).await
    }
}
