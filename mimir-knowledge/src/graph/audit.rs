use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Audit log delegates
    // ------------------------------------------------------------------

    /// Query the audit log with optional filters.
    pub async fn query_audit_log(
        &self,
        filter: queries::audit::AuditLogFilter,
    ) -> Result<Vec<queries::audit::AuditLogRow>, KnowledgeError> {
        queries::audit::query_audit_log(&self.pool, &filter).await
    }
}
