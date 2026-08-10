//! Scheduled nightly optimization entry point.

use crate::KnowledgeGraph;

use super::{OptimizationConfig, OptimizationRunner};

pub async fn run_nightly_optimization(kg: &KnowledgeGraph) -> Result<(), crate::KnowledgeError> {
    let backup_dir = mimir_core::paths::data_dir()
        .map(|p| p.join("backups"))
        .map_err(|e| crate::KnowledgeError::Validation(e.to_string()))?;
    let runner = OptimizationRunner::new(kg, OptimizationConfig::for_test(backup_dir), None);
    runner.run_all().await?;
    Ok(())
}
