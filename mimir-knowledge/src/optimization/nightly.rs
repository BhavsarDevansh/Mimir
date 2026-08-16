//! Scheduled nightly optimization entry point.

use std::path::Path;

use crate::KnowledgeGraph;

use super::{OptimizationConfig, OptimizationRunner};

/// Run the full nightly optimization pipeline against `kg`, writing backups
/// into `backup_dir`.
///
/// Callers must supply an isolated backup directory (tests use a per-test
/// tempdir) so concurrent runs never share state; the shared real data
/// directory is owned by the daemon's scheduled job.
pub async fn run_nightly_optimization(
    kg: &KnowledgeGraph,
    backup_dir: &Path,
) -> Result<(), crate::KnowledgeError> {
    let runner = OptimizationRunner::new(
        kg,
        OptimizationConfig::for_test(backup_dir.to_path_buf()),
        None,
    );
    runner.run_all().await?;
    Ok(())
}
